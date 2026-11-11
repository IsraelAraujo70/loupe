//! Saved views, after the t3code kanban's `KanbanView`: a named set of filter
//! clauses plus display settings, shown as a tab on team screens. A view
//! lives in Linear as a custom view: [`import_linear_filter`] and
//! [`linear_preferences`] read one, [`export_linear_filter`] and
//! [`export_preferences`] write one, and the pairs round-trip. The JSON file
//! of views Loupe once kept on its own ([`ViewsFile`]) is still read, so the
//! app can offer to move what is in it to Linear.

use std::path::Path;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::filter::{
    CURRENT_CYCLE, DateField, DateOp, FilterClause, FlagField, ME, NONE, SetField, SetOp,
};
use crate::types::{ClosedIssues, IssueQuery, Label, LinearCustomView, Team, WorkflowState};

/// The file format version [`save`] writes.
pub const VIEWS_VERSION: u32 = 1;
pub const MAX_CLAUSES: usize = 30;
pub const MAX_VALUES: usize = 50;
pub const MAX_NAME: usize = 60;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Layout {
    #[default]
    Board,
    List,
}

/// What board columns and list sections group by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Grouping {
    #[default]
    Status,
    Assignee,
    Priority,
    Label,
    Project,
    Cycle,
    None,
}

impl Grouping {
    pub const ALL: [Grouping; 7] = [
        Grouping::Status,
        Grouping::Assignee,
        Grouping::Priority,
        Grouping::Label,
        Grouping::Project,
        Grouping::Cycle,
        Grouping::None,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Grouping::Status => "Status",
            Grouping::Assignee => "Assignee",
            Grouping::Priority => "Priority",
            Grouping::Label => "Label",
            Grouping::Project => "Project",
            Grouping::Cycle => "Cycle",
            Grouping::None => "No grouping",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Ordering {
    /// Urgent first, no priority last, then newest update.
    #[default]
    Priority,
    Updated,
    Created,
    /// Soonest first, none last.
    DueDate,
    /// Linear's manual order, which dragging a card on the board changes.
    Manual,
}

impl Ordering {
    pub const ALL: [Ordering; 5] = [
        Ordering::Manual,
        Ordering::Priority,
        Ordering::Updated,
        Ordering::Created,
        Ordering::DueDate,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Ordering::Priority => "Priority",
            Ordering::Updated => "Last updated",
            Ordering::Created => "Created",
            Ordering::DueDate => "Due date",
            Ordering::Manual => "Manual",
        }
    }
}

/// Explicit direction saved by Linear; absent means the ordering's default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// Which closed issues a view shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompletedWindow {
    All,
    PastWeek,
    #[default]
    PastMonth,
    None,
}

impl CompletedWindow {
    pub const ALL: [CompletedWindow; 4] = [
        CompletedWindow::All,
        CompletedWindow::PastWeek,
        CompletedWindow::PastMonth,
        CompletedWindow::None,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CompletedWindow::All => "All",
            CompletedWindow::PastWeek => "Past week",
            CompletedWindow::PastMonth => "Past month",
            CompletedWindow::None => "None",
        }
    }

    pub fn closed_issues(self, now: DateTime<Utc>) -> ClosedIssues {
        match self {
            CompletedWindow::All => ClosedIssues::All,
            CompletedWindow::PastWeek => ClosedIssues::ClosedSince(now - Duration::days(7)),
            CompletedWindow::PastMonth => ClosedIssues::ClosedSince(now - Duration::days(30)),
            CompletedWindow::None => ClosedIssues::None,
        }
    }
}

/// Card and row properties a view shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DisplayProperty {
    Id,
    Status,
    Assignee,
    Priority,
    Project,
    DueDate,
    Cycle,
    Labels,
    Estimate,
    PullRequests,
    Created,
    Updated,
}

impl DisplayProperty {
    pub const ALL: [DisplayProperty; 12] = [
        DisplayProperty::Id,
        DisplayProperty::Status,
        DisplayProperty::Assignee,
        DisplayProperty::Priority,
        DisplayProperty::Project,
        DisplayProperty::DueDate,
        DisplayProperty::Cycle,
        DisplayProperty::Labels,
        DisplayProperty::Estimate,
        DisplayProperty::PullRequests,
        DisplayProperty::Created,
        DisplayProperty::Updated,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DisplayProperty::Id => "ID",
            DisplayProperty::Status => "Status",
            DisplayProperty::Assignee => "Assignee",
            DisplayProperty::Priority => "Priority",
            DisplayProperty::Project => "Project",
            DisplayProperty::DueDate => "Due date",
            DisplayProperty::Cycle => "Cycle",
            DisplayProperty::Labels => "Labels",
            DisplayProperty::Estimate => "Estimate",
            DisplayProperty::PullRequests => "Pull requests",
            DisplayProperty::Created => "Created",
            DisplayProperty::Updated => "Updated",
        }
    }
}

/// A view's filters and display settings. Missing fields take defaults, so
/// older files keep loading.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ViewConfig {
    pub filters: Vec<FilterClause>,
    pub layout: Layout,
    pub group_by: Grouping,
    pub order_by: Ordering,
    pub sort_direction: Option<SortDirection>,
    pub completed: CompletedWindow,
    /// Board columns with no issues stay visible.
    pub show_empty_columns: bool,
    pub properties: Vec<DisplayProperty>,
    /// Board columns and list sections the user hid, by group key (a state
    /// id when grouped by status).
    pub hidden_groups: Vec<String>,
    /// The board's columns in the order the user dragged them into, by
    /// group key. Empty: the grouping's own order. A column it does not
    /// name comes after the ones it does, in the grouping's order.
    pub column_order: Vec<String>,
    /// Sub-issues are listed beside their parents. A display setting in
    /// Linear, so it is not one of `filters`.
    pub show_sub_issues: bool,
}

impl Default for ViewConfig {
    fn default() -> Self {
        Self {
            filters: Vec::new(),
            layout: Layout::Board,
            group_by: Grouping::Status,
            order_by: Ordering::Priority,
            sort_direction: None,
            completed: CompletedWindow::PastMonth,
            show_empty_columns: true,
            hidden_groups: Vec::new(),
            column_order: Vec::new(),
            show_sub_issues: true,
            properties: vec![
                DisplayProperty::Id,
                DisplayProperty::Status,
                DisplayProperty::Assignee,
                DisplayProperty::Priority,
                DisplayProperty::Project,
                DisplayProperty::DueDate,
                DisplayProperty::Labels,
                DisplayProperty::PullRequests,
                DisplayProperty::Updated,
            ],
        }
    }
}

impl ViewConfig {
    /// What to ask Linear for at `now`.
    pub fn query(&self, now: DateTime<Utc>) -> IssueQuery {
        let mut clauses = self.filters.clone();
        if !self.show_sub_issues {
            clauses.push(FilterClause::Flag {
                field: FlagField::IsSubIssue,
                value: false,
            });
        }
        IssueQuery {
            clauses,
            closed: self.completed.closed_issues(now),
        }
    }

    pub fn shows(&self, property: DisplayProperty) -> bool {
        self.properties.contains(&property)
    }
}

/// A saved view, shown as a tab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub id: String,
    /// `None`: on every team's screen; otherwise on that team's only.
    #[serde(default)]
    pub team_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub config: ViewConfig,
    /// The Linear custom view it came from, so a second import skips it.
    #[serde(default)]
    pub linear_view_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Everything in `linear-views.json`, views in tab order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewsFile {
    pub version: u32,
    pub views: Vec<View>,
}

fn set(field: SetField, values: &[&str]) -> FilterClause {
    FilterClause::Set {
        field,
        op: SetOp::In,
        values: values.iter().map(|value| (*value).to_owned()).collect(),
    }
}

impl ViewsFile {
    /// The views a new file starts with, on every team: the same set t3code
    /// seeds.
    pub fn seeded(now: DateTime<Utc>) -> ViewsFile {
        let view = |id: &str, name: &str, config: ViewConfig| View {
            id: id.to_owned(),
            team_id: None,
            name: name.to_owned(),
            config,
            linear_view_id: None,
            created_at: now,
            updated_at: now,
        };
        let filtered = |filters| ViewConfig {
            filters,
            ..ViewConfig::default()
        };
        ViewsFile {
            version: VIEWS_VERSION,
            views: vec![
                view(
                    "v_active",
                    "Active",
                    filtered(vec![set(SetField::StatusType, &["unstarted", "started"])]),
                ),
                view(
                    "v_backlog",
                    "Backlog",
                    filtered(vec![set(SetField::StatusType, &["triage", "backlog"])]),
                ),
                view("v_all", "All issues", ViewConfig::default()),
                view(
                    "v_mine",
                    "My issues",
                    filtered(vec![set(SetField::Assignee, &[ME])]),
                ),
                view(
                    "v_done",
                    "Done",
                    ViewConfig {
                        completed: CompletedWindow::All,
                        ..filtered(vec![set(
                            SetField::StatusType,
                            &["completed", "canceled", "duplicate"],
                        )])
                    },
                ),
            ],
        }
    }

    /// The views a team's screen shows, in tab order.
    pub fn for_team<'a>(&'a self, team_id: &'a str) -> impl Iterator<Item = &'a View> + 'a {
        self.views
            .iter()
            .filter(move |view| view.team_id.as_deref().is_none_or(|id| id == team_id))
    }

    pub fn get(&self, id: &str) -> Option<&View> {
        self.views.iter().find(|view| view.id == id)
    }

    fn fresh_id(&self, now: DateTime<Utc>) -> String {
        let base = now.timestamp_nanos_opt().unwrap_or_default();
        (0u64..)
            .map(|n| format!("v_{:x}", base as u64 + n))
            .find(|id| self.get(id).is_none())
            .unwrap_or_else(|| format!("v_{base:x}"))
    }

    /// Adds a view at the end and returns its id. The name is trimmed and
    /// cut to [`MAX_NAME`] characters; a blank name becomes "Untitled view".
    pub fn create(
        &mut self,
        name: &str,
        team_id: Option<String>,
        config: ViewConfig,
        now: DateTime<Utc>,
    ) -> String {
        let id = self.fresh_id(now);
        self.views.push(View {
            id: id.clone(),
            team_id,
            name: clean_name(name),
            config,
            linear_view_id: None,
            created_at: now,
            updated_at: now,
        });
        id
    }

    /// Whether the view exists.
    pub fn rename(&mut self, id: &str, name: &str, now: DateTime<Utc>) -> bool {
        self.edit(id, now, |view| view.name = clean_name(name))
    }

    pub fn update_config(&mut self, id: &str, config: ViewConfig, now: DateTime<Utc>) -> bool {
        self.edit(id, now, |view| view.config = config)
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.views.len();
        self.views.retain(|view| view.id != id);
        self.views.len() != before
    }

    /// Moves a view one place earlier (`-1`) or later (`1`) among the views
    /// `team_id`'s screen shows, so the tabs the user sees swap.
    pub fn move_by(&mut self, id: &str, team_id: &str, step: i32) -> bool {
        let visible: Vec<usize> = self
            .views
            .iter()
            .enumerate()
            .filter(|(_, view)| view.team_id.as_deref().is_none_or(|t| t == team_id))
            .map(|(index, _)| index)
            .collect();
        let Some(position) = visible.iter().position(|&index| self.views[index].id == id) else {
            return false;
        };
        let target = position as i64 + i64::from(step);
        if target < 0 || target as usize >= visible.len() {
            return false;
        }
        self.views.swap(visible[position], visible[target as usize]);
        true
    }

    fn edit(&mut self, id: &str, now: DateTime<Utc>, change: impl FnOnce(&mut View)) -> bool {
        match self.views.iter_mut().find(|view| view.id == id) {
            Some(view) => {
                change(view);
                view.updated_at = now;
                true
            }
            None => false,
        }
    }
}

fn clean_name(name: &str) -> String {
    let name: String = name.trim().chars().take(MAX_NAME).collect();
    if name.is_empty() {
        "Untitled view".to_owned()
    } else {
        name
    }
}

/// What [`load`] found.
#[derive(Clone, Debug, PartialEq)]
pub enum Loaded {
    /// The file parsed.
    Read(ViewsFile),
    /// No file yet: the seeded views.
    Seeded(ViewsFile),
    /// The file did not parse; it was copied to `<file>.bak` and the seeded
    /// views are used, so the next save does not lose it.
    Recovered { file: ViewsFile, reason: String },
}

impl Loaded {
    pub fn into_file(self) -> ViewsFile {
        match self {
            Loaded::Read(file) | Loaded::Seeded(file) | Loaded::Recovered { file, .. } => file,
        }
    }
}

/// Reads the views file at `path`, tolerating a missing, corrupt, or newer
/// file.
pub fn load(path: &Path, now: DateTime<Utc>) -> Loaded {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Loaded::Seeded(ViewsFile::seeded(now));
        }
        Err(error) => {
            return Loaded::Recovered {
                file: ViewsFile::seeded(now),
                reason: error.to_string(),
            };
        }
    };
    let reason = match serde_json::from_slice::<ViewsFile>(&bytes) {
        Ok(file) if file.version <= VIEWS_VERSION => return Loaded::Read(file),
        Ok(file) => format!("version {} is newer than {VIEWS_VERSION}", file.version),
        Err(error) => error.to_string(),
    };
    let mut backup = path.as_os_str().to_owned();
    backup.push(".bak");
    let _ = std::fs::write(&backup, &bytes);
    Loaded::Recovered {
        file: ViewsFile::seeded(now),
        reason,
    }
}

/// Writes the views file atomically (a temporary file renamed over it).
pub fn save(path: &Path, file: &ViewsFile) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let bytes = serde_json::to_vec_pretty(&ViewsFile {
        version: VIEWS_VERSION,
        views: file.views.clone(),
    })
    .map_err(std::io::Error::other)?;
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
}

/// What a Linear view's labels and state names resolve against.
pub struct ImportContext<'a> {
    /// The team's and the workspace's labels.
    pub labels: &'a [Label],
    pub states: &'a [WorkflowState],
}

struct Unsupported(String);

fn unsupported(what: &str) -> Unsupported {
    Unsupported(format!("{what}: not supported in Loupe"))
}

fn only_entry<'a>(field: &str, value: &'a Value) -> Result<(&'a String, &'a Value), Unsupported> {
    let object = value.as_object().ok_or_else(|| unsupported(field))?;
    let mut entries = object.iter();
    match (entries.next(), entries.next()) {
        (Some(entry), None) => Ok(entry),
        _ => Err(unsupported(field)),
    }
}

fn set_clause(
    field: SetField,
    values: Vec<String>,
    op: SetOp,
) -> Result<FilterClause, Unsupported> {
    let mut unique: Vec<String> = Vec::new();
    for value in values {
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    if unique.len() > MAX_VALUES {
        return Err(unsupported(&format!(
            "{} with so many values",
            field.label()
        )));
    }
    Ok(FilterClause::Set {
        field,
        op,
        values: unique,
    })
}

/// `{ eq: x }`, `{ in: [...] }`, or `{ nin: [...] }` with string (or, for
/// priority, number) values.
fn comparison(
    field: &str,
    value: &Value,
    allow_nin: bool,
) -> Result<(SetOp, Vec<String>), Unsupported> {
    let (op, operand) = only_entry(field, value)?;
    let list: Vec<&Value> = match op.as_str() {
        "eq" => vec![operand],
        "in" | "nin" => operand
            .as_array()
            .map(|items| items.iter().collect())
            .ok_or_else(|| unsupported(field))?,
        _ => return Err(unsupported(field)),
    };
    if list.is_empty() || (op == "nin" && !allow_nin) {
        return Err(unsupported(field));
    }
    let values = list
        .into_iter()
        .map(|item| match item {
            Value::String(text) => Ok(text.clone()),
            Value::Number(number) if field == "priority" => number
                .as_i64()
                .map(|n| n.to_string())
                .ok_or_else(|| unsupported(field)),
            _ => Err(unsupported(field)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let op = if op == "nin" { SetOp::NotIn } else { SetOp::In };
    Ok((op, values))
}

fn by_name(
    field: &str,
    value: &Value,
    lookup: impl Fn(&str) -> Vec<String>,
) -> Result<Vec<String>, Unsupported> {
    let (_, names) = comparison(field, value, false)?;
    let mut ids = Vec::new();
    for name in names {
        let found = lookup(&name);
        if found.is_empty() {
            return Err(Unsupported(format!(
                "{field} \"{name}\": not found in Loupe"
            )));
        }
        ids.extend(found);
    }
    Ok(ids)
}

fn null_flag(field: &str, value: &Value) -> Result<bool, Unsupported> {
    match only_entry(field, value)? {
        (key, Value::Bool(is_null)) if key == "null" => Ok(!is_null),
        _ => Err(unsupported(field)),
    }
}

fn person_clause(field: SetField, name: &str, value: &Value) -> Result<FilterClause, Unsupported> {
    let (key, operand) = only_entry(name, value)?;
    match key.as_str() {
        "isMe" => match only_entry(name, operand)? {
            (op, Value::Bool(true)) if op == "eq" => set_clause(field, vec![ME.into()], SetOp::In),
            _ => Err(unsupported(name)),
        },
        "null" => none_clause(field, name, operand),
        "id" => {
            let (_, ids) = comparison(name, operand, false)?;
            set_clause(field, ids, SetOp::In)
        }
        other => Err(unsupported(&format!("{name}.{other}"))),
    }
}

/// `{ null: true }` is "has none"; `{ null: false }` is "has one".
fn none_clause(field: SetField, name: &str, operand: &Value) -> Result<FilterClause, Unsupported> {
    match operand {
        Value::Bool(true) => set_clause(field, vec![NONE.into()], SetOp::In),
        Value::Bool(false) => set_clause(field, vec![NONE.into()], SetOp::NotIn),
        _ => Err(unsupported(name)),
    }
}

/// `{ lt: day }` or `{ gt: day }`, where a day is `2026-11-01` or the
/// midnight UTC that starts it: the two date conditions Loupe writes.
fn date_clause(field: DateField, name: &str, value: &Value) -> Result<FilterClause, Unsupported> {
    let (key, operand) = only_entry(name, value)?;
    let text = operand.as_str().ok_or_else(|| unsupported(name))?;
    let day = text
        .strip_suffix("T00:00:00.000Z")
        .unwrap_or(text)
        .parse::<NaiveDate>()
        .map_err(|_| unsupported(&format!("{name} with a time or a relative date")))?;
    let op = match key.as_str() {
        "lt" => DateOp::Before(day),
        "gt" => DateOp::After(day),
        other => return Err(unsupported(&format!("{name}.{other}"))),
    };
    Ok(FilterClause::Date { field, op })
}

fn labels_by_name(context: &ImportContext<'_>, name: &str) -> Vec<String> {
    let lower = name.to_lowercase();
    let mut ids: Vec<String> = Vec::new();
    for label in context.labels {
        if label.name.to_lowercase() == lower {
            ids.push(label.id.clone());
            // A label group's name also stands for every label inside it.
            ids.extend(
                context
                    .labels
                    .iter()
                    .filter(|child| child.parent_id.as_deref() == Some(label.id.as_str()))
                    .map(|child| child.id.clone()),
            );
        }
    }
    ids
}

fn field_clauses(
    field: &str,
    value: &Value,
    context: &ImportContext<'_>,
) -> Result<Vec<FilterClause>, Unsupported> {
    if !value.is_object() {
        return Err(unsupported(field));
    }
    let (key, operand) = only_entry(field, value)?;
    if key == "and" || key == "or" {
        let branches = operand
            .as_array()
            .filter(|items| !items.is_empty())
            .ok_or_else(|| unsupported(field))?;
        if key == "and" {
            let mut clauses = Vec::new();
            for branch in branches {
                clauses.extend(field_clauses(field, branch, context)?);
            }
            return Ok(clauses);
        }
        let mut alternatives = Vec::new();
        for branch in branches {
            let clauses = field_clauses(field, branch, context)?;
            // Flattening an `and` inside an `or` would widen the view.
            if clauses.len() != 1 {
                return Err(unsupported(&format!("{field} with \"and\" inside \"or\"")));
            }
            alternatives.extend(clauses);
        }
        return Ok(vec![merge_alternatives(field, alternatives)?]);
    }
    let clause = match field {
        "assignee" => person_clause(SetField::Assignee, field, value)?,
        "creator" => person_clause(SetField::Creator, field, value)?,
        "state" => match key.as_str() {
            "type" => {
                let (op, values) = comparison("state", operand, true)?;
                set_clause(SetField::StatusType, values, op)?
            }
            "name" => {
                let ids = by_name("state", operand, |name| {
                    context
                        .states
                        .iter()
                        .filter(|state| state.name.eq_ignore_ascii_case(name))
                        .map(|state| state.id.clone())
                        .collect()
                })?;
                set_clause(SetField::Status, ids, SetOp::In)?
            }
            "id" => {
                let (op, ids) = comparison("state", operand, true)?;
                set_clause(SetField::Status, ids, op)?
            }
            other => return Err(unsupported(&format!("state.{other}"))),
        },
        "priority" => {
            let (op, values) = comparison("priority", value, true)?;
            set_clause(SetField::Priority, values, op)?
        }
        "labels" => {
            let (name_key, name_value) = if key == "parent" {
                only_entry("labels", operand)?
            } else {
                (key, operand)
            };
            if name_key == "length" {
                // No label at all.
                return match only_entry("labels", name_value)? {
                    (op, count) if op == "eq" && count.as_i64() == Some(0) => Ok(vec![set_clause(
                        SetField::Label,
                        vec![NONE.into()],
                        SetOp::In,
                    )?]),
                    _ => Err(unsupported("labels.length")),
                };
            }
            let name_value = if name_key == "some" {
                let (inner_key, inner) = only_entry("labels", name_value)?;
                if inner_key == "id" {
                    let (_, ids) = comparison("label", inner, false)?;
                    return Ok(vec![set_clause(SetField::Label, ids, SetOp::In)?]);
                }
                if inner_key != "name" {
                    return Err(unsupported(&format!("labels.some.{inner_key}")));
                }
                inner
            } else if name_key == "name" {
                name_value
            } else {
                return Err(unsupported(&format!("labels.{key}")));
            };
            let ids = by_name("label", name_value, |name| labels_by_name(context, name))?;
            set_clause(SetField::Label, ids, SetOp::In)?
        }
        "project" | "cycle" | "projectMilestone" => {
            let set_field = match field {
                "project" => SetField::Project,
                "cycle" => SetField::Cycle,
                _ => SetField::Milestone,
            };
            match key.as_str() {
                "id" => {
                    let (_, ids) = comparison(field, operand, false)?;
                    set_clause(set_field, ids, SetOp::In)?
                }
                "null" => none_clause(set_field, field, operand)?,
                "isActive" if field == "cycle" => match only_entry(field, operand)? {
                    (op, Value::Bool(true)) if op == "eq" => {
                        set_clause(set_field, vec![CURRENT_CYCLE.into()], SetOp::In)?
                    }
                    _ => return Err(unsupported("cycle.isActive")),
                },
                other => return Err(unsupported(&format!("{field}.{other}"))),
            }
        }
        "parent" => FilterClause::Flag {
            field: FlagField::IsSubIssue,
            value: null_flag(field, value)?,
        },
        "dueDate" if key == "null" => FilterClause::Flag {
            field: FlagField::HasDueDate,
            value: null_flag(field, value)?,
        },
        "dueDate" => date_clause(DateField::DueDate, field, value)?,
        "createdAt" => date_clause(DateField::CreatedAt, field, value)?,
        "updatedAt" => date_clause(DateField::UpdatedAt, field, value)?,
        "children" => {
            if key != "length" {
                return Err(unsupported(&format!("children.{key}")));
            }
            let (op, count) = only_entry(field, operand)?;
            let has = match (op.as_str(), count.as_i64()) {
                ("gt", Some(0)) => true,
                ("eq", Some(0)) => false,
                _ => return Err(unsupported(field)),
            };
            FilterClause::Flag {
                field: FlagField::HasSubIssues,
                value: has,
            }
        }
        other => return Err(unsupported(other)),
    };
    Ok(vec![clause])
}

/// Linear writes "is any of" as an `or` of single matches on one field.
fn merge_alternatives(
    field: &str,
    clauses: Vec<FilterClause>,
) -> Result<FilterClause, Unsupported> {
    let mut merged_field = None;
    let mut values = Vec::new();
    for clause in clauses {
        match clause {
            FilterClause::Set {
                field: set_field,
                op: SetOp::In,
                values: more,
            } if merged_field.is_none_or(|known| known == set_field) => {
                merged_field = Some(set_field);
                values.extend(more);
            }
            _ => return Err(unsupported(&format!("{field} with \"or\""))),
        }
    }
    let set_field = merged_field.ok_or_else(|| unsupported(field))?;
    set_clause(set_field, values, SetOp::In)
}

fn conjunction(
    value: &Value,
    context: &ImportContext<'_>,
    clauses: &mut Vec<FilterClause>,
) -> Result<(), Unsupported> {
    let object: &Map<String, Value> = value.as_object().ok_or_else(|| unsupported("filter"))?;
    // Sorted explicitly: serde_json's map order depends on whether some crate
    // in the build enables `preserve_order`, and chip order must not.
    let mut fields: Vec<(&String, &Value)> = object.iter().collect();
    fields.sort_by(|a, b| a.0.cmp(b.0));
    for (field, operand) in fields {
        if field == "and" {
            let parts = operand.as_array().ok_or_else(|| unsupported("and"))?;
            for part in parts {
                conjunction(part, context, clauses)?;
            }
        } else {
            clauses.extend(field_clauses(field, operand, context)?);
        }
    }
    Ok(())
}

/// Turns a Linear custom view's `filterData` into clauses. Only filters Loupe
/// reproduces exactly are imported; anything else rejects the whole view with
/// a reason, so an import never shows more issues than the view it came
/// from.
pub fn import_linear_filter(
    filter_data: &Value,
    context: &ImportContext<'_>,
) -> Result<Vec<FilterClause>, String> {
    let mut clauses = Vec::new();
    conjunction(filter_data, context, &mut clauses).map_err(|Unsupported(reason)| reason)?;
    if clauses.len() > MAX_CLAUSES {
        return Err("so many filters: not supported in Loupe".into());
    }
    Ok(clauses)
}

/// What a clause is called in a message about it.
fn clause_name(clause: &FilterClause) -> &'static str {
    match clause {
        FilterClause::Set { field, .. } => field.label(),
        FilterClause::Date { field, .. } => field.label(),
        FilterClause::Flag { field, .. } => field.label(),
        FilterClause::Text { .. } => "Search text",
    }
}

/// One alternative per value, in the values' order, inside the field's own
/// `or`: the form Linear's app writes and [`import_linear_filter`] reads
/// back in the same order.
fn alternatives(field: &str, each: Vec<Value>) -> Option<Value> {
    let mut each = each;
    match each.len() {
        0 => None,
        1 => each.pop().map(|only| json!({ field: only })),
        _ => Some(json!({ field: { "or": each } })),
    }
}

/// The exact `IssueFilter` of one clause, or `None` when Linear has no
/// filter that keeps the same issues.
fn export_clause(clause: &FilterClause) -> Option<Value> {
    let day = |date: &NaiveDate| date.format("%Y-%m-%d").to_string();
    match clause {
        FilterClause::Set { values, .. } if values.is_empty() => None,
        FilterClause::Set { field, op, values } => {
            let key = if *op == SetOp::In { "in" } else { "nin" };
            // A relation that may be missing: ids, "none", and for people
            // "me". Linear does not say what `nin` does with a missing
            // relation, so only "has one" (`null: false`) is written negated.
            let relation = |name: &str, person: bool, current: bool| {
                if *op == SetOp::NotIn {
                    return (values.len() == 1 && values[0] == NONE)
                        .then(|| json!({ name: { "null": false } }));
                }
                let each = values
                    .iter()
                    .map(|value| match value.as_str() {
                        NONE => Some(json!({ "null": true })),
                        ME if person => Some(json!({ "isMe": { "eq": true } })),
                        CURRENT_CYCLE if current => Some(json!({ "isActive": { "eq": true } })),
                        ME | CURRENT_CYCLE => None,
                        id => Some(json!({ "id": { "in": [id] } })),
                    })
                    .collect::<Option<Vec<Value>>>()?;
                alternatives(name, each)
            };
            match field {
                SetField::Status => Some(json!({ "state": { "id": { key: values } } })),
                SetField::StatusType => Some(json!({ "state": { "type": { key: values } } })),
                SetField::Priority => {
                    let numbers = values
                        .iter()
                        .map(|value| value.parse::<i64>().ok())
                        .collect::<Option<Vec<i64>>>()?;
                    Some(json!({ "priority": { key: numbers } }))
                }
                SetField::Assignee => relation("assignee", true, false),
                SetField::Creator => relation("creator", true, false),
                SetField::Project => relation("project", false, false),
                SetField::Milestone => relation("projectMilestone", false, false),
                SetField::Cycle => relation("cycle", false, true),
                SetField::Label if *op == SetOp::In => {
                    let each = values
                        .iter()
                        .map(|value| match value.as_str() {
                            NONE => json!({ "length": { "eq": 0 } }),
                            id => json!({ "some": { "id": { "in": [id] } } }),
                        })
                        .collect();
                    alternatives("labels", each)
                }
                // A pull request's state lives in attachment metadata,
                // which Linear cannot filter by.
                SetField::Label | SetField::PullRequest => None,
            }
        }
        FilterClause::Date { field, op } => {
            let name = match field {
                DateField::CreatedAt => "createdAt",
                DateField::UpdatedAt => "updatedAt",
                DateField::DueDate => "dueDate",
                // Closed is two dates in Linear (completed, canceled).
                DateField::ClosedAt => return None,
            };
            let stamp = |date: &NaiveDate| match field {
                DateField::DueDate => day(date),
                _ => format!("{}T00:00:00.000Z", day(date)),
            };
            match op {
                DateOp::Before(date) => Some(json!({ name: { "lt": stamp(date) } })),
                DateOp::After(date) => Some(json!({ name: { "gt": stamp(date) } })),
                // A window from "now" would be frozen at the day it was saved.
                DateOp::PastDays(_) | DateOp::NextDays(_) | DateOp::IsSet | DateOp::IsEmpty => None,
            }
        }
        FilterClause::Flag { field, value } => match field {
            FlagField::IsSubIssue => Some(json!({ "parent": { "null": !value } })),
            FlagField::HasDueDate => Some(json!({ "dueDate": { "null": !value } })),
            FlagField::HasSubIssues if *value => {
                Some(json!({ "children": { "length": { "gt": 0 } } }))
            }
            FlagField::HasSubIssues => Some(json!({ "children": { "length": { "eq": 0 } } })),
            FlagField::HasPullRequests => None,
        },
        FilterClause::Text { .. } => None,
    }
}

/// Turns clauses into the `filterData` of a Linear custom view. Every clause
/// is written exactly or the whole filter is refused with the clause's name,
/// so a view saved in Linear never lists other issues than it lists here.
/// [`import_linear_filter`] reads the result back as the same clauses.
pub fn export_linear_filter(clauses: &[FilterClause]) -> Result<Value, String> {
    let parts = clauses
        .iter()
        .map(|clause| {
            export_clause(clause).ok_or_else(|| {
                format!(
                    "Linear cannot save the filter \"{}\" as it is set here",
                    clause_name(clause)
                )
            })
        })
        .collect::<Result<Vec<Value>, String>>()?;
    Ok(if parts.is_empty() {
        json!({})
    } else {
        json!({ "and": parts })
    })
}

/// The preferences object to save for a Linear view so that it shows as
/// `config`: `own`, what the key's owner already had for the view (Linear
/// replaces the whole object on a save), with the settings of `config` that
/// differ from `saved` written over it. A setting the user did not change is
/// not written, so one Loupe reads as a default (a grouping it does not
/// have) or reads from another key (empty groups of the other layout) stays
/// as Linear has it. With no `saved`, for a new view, every setting is
/// written. The inverse of [`linear_preferences`]: reading the result back
/// gives `config`'s display settings.
pub fn export_preferences(config: &ViewConfig, saved: Option<&ViewConfig>, own: &Value) -> Value {
    let mut preferences = own.as_object().cloned().unwrap_or_default();
    let mut set = |key: &str, value: Value| {
        preferences.insert(key.to_owned(), value);
    };
    let changed = |differs: fn(&ViewConfig, &ViewConfig) -> bool| {
        saved.is_none_or(|saved| differs(config, saved))
    };
    if changed(|a, b| a.layout != b.layout) {
        set(
            "layout",
            json!(match config.layout {
                Layout::Board => "board",
                Layout::List => "list",
            }),
        );
    }
    if changed(|a, b| a.group_by != b.group_by) {
        set(
            "issueGrouping",
            json!(match config.group_by {
                Grouping::Status => "workflowState",
                Grouping::Assignee => "assignee",
                Grouping::Priority => "priority",
                Grouping::Label => "label",
                Grouping::Project => "project",
                Grouping::Cycle => "cycle",
                Grouping::None => "none",
            }),
        );
    }
    if changed(|a, b| a.order_by != b.order_by) {
        set(
            "viewOrdering",
            json!(match config.order_by {
                Ordering::Priority => "priority",
                Ordering::Updated => "updatedAt",
                Ordering::Created => "createdAt",
                Ordering::DueDate => "dueDate",
                Ordering::Manual => "manual",
            }),
        );
    }
    if changed(|a, b| a.sort_direction != b.sort_direction) {
        set(
            "viewOrderingDirection",
            match config.sort_direction {
                Some(SortDirection::Ascending) => json!("asc"),
                Some(SortDirection::Descending) => json!("desc"),
                None => Value::Null,
            },
        );
    }
    if changed(|a, b| a.completed != b.completed) {
        set(
            "showCompletedIssues",
            json!(match config.completed {
                CompletedWindow::All => "all",
                CompletedWindow::PastWeek => "pastWeek",
                CompletedWindow::PastMonth => "pastMonth",
                CompletedWindow::None => "none",
            }),
        );
    }
    // Both are kept per layout in Linear: only the layout on screen is
    // written (and, for a new view, the setting both layouts fall back to).
    if changed(|a, b| a.show_empty_columns != b.show_empty_columns || a.layout != b.layout) {
        set(
            empty_groups_key(config.layout),
            json!(config.show_empty_columns),
        );
        if saved.is_none() {
            set("showEmptyGroups", json!(config.show_empty_columns));
        }
    }
    if changed(|a, b| a.hidden_groups != b.hidden_groups || a.layout != b.layout) {
        set(
            hidden_groups_key(config.layout),
            json!(config.hidden_groups),
        );
    }
    if changed(|a, b| a.column_order != b.column_order) {
        set("columnOrderBoard", json!(config.column_order));
    }
    if changed(|a, b| a.show_sub_issues != b.show_sub_issues) {
        set("showSubIssues", json!(config.show_sub_issues));
    }
    for (key, property) in PROPERTY_FIELDS {
        let shown = config.properties.contains(&property);
        if saved.is_none_or(|saved| saved.properties.contains(&property) != shown) {
            set(key, json!(shown));
        }
    }
    Value::Object(preferences)
}

/// Where Linear keeps whether a layout shows its empty groups.
fn empty_groups_key(layout: Layout) -> &'static str {
    match layout {
        Layout::Board => "showEmptyGroupsBoard",
        Layout::List => "showEmptyGroupsList",
    }
}

/// Where Linear keeps the hidden groups of a layout.
fn hidden_groups_key(layout: Layout) -> &'static str {
    match layout {
        Layout::Board => "hiddenColumns",
        Layout::List => "hiddenGroupsList",
    }
}

/// Linear's name for the setting that shows each card and row property.
const PROPERTY_FIELDS: [(&str, DisplayProperty); 12] = [
    ("fieldId", DisplayProperty::Id),
    ("fieldStatus", DisplayProperty::Status),
    ("fieldAssignee", DisplayProperty::Assignee),
    ("fieldPriority", DisplayProperty::Priority),
    ("fieldProject", DisplayProperty::Project),
    ("fieldDueDate", DisplayProperty::DueDate),
    ("fieldCycle", DisplayProperty::Cycle),
    ("fieldLabels", DisplayProperty::Labels),
    ("fieldEstimate", DisplayProperty::Estimate),
    ("fieldPullRequests", DisplayProperty::PullRequests),
    ("fieldDateCreated", DisplayProperty::Created),
    ("fieldDateUpdated", DisplayProperty::Updated),
];

/// What [`import_views`] did, by view name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportReport {
    pub imported: Vec<String>,
    /// Views already imported earlier.
    pub already: Vec<String>,
    /// Views that use filters Loupe cannot reproduce, with the reason.
    pub skipped: Vec<(String, String)>,
}

/// Adds the Linear views that apply to `team` (its own and workspace views)
/// to `file`, each once. Reads Linear's views only; nothing is written back.
pub fn import_views(
    file: &mut ViewsFile,
    linear_views: &[LinearCustomView],
    team: &Team,
    now: DateTime<Utc>,
) -> ImportReport {
    let context = ImportContext {
        labels: &team.labels,
        states: &team.states,
    };
    let mut report = ImportReport::default();
    for linear in linear_views {
        if linear.team_id.as_deref().is_some_and(|id| id != team.id) {
            continue;
        }
        if file
            .views
            .iter()
            .any(|view| view.linear_view_id.as_deref() == Some(linear.id.as_str()))
        {
            report.already.push(linear.name.clone());
            continue;
        }
        match import_linear_filter(&linear.filter_data, &context) {
            Ok(filters) => {
                let id = file.create(
                    &linear.name,
                    linear.team_id.clone(),
                    ViewConfig {
                        filters,
                        ..ViewConfig::default()
                    },
                    now,
                );
                if let Some(view) = file.views.iter_mut().find(|view| view.id == id) {
                    view.linear_view_id = Some(linear.id.clone());
                }
                report.imported.push(linear.name.clone());
            }
            Err(reason) => report.skipped.push((linear.name.clone(), reason)),
        }
    }
    report
}

/// Converts Linear display settings into a view's, with no filters.
/// Unrepresentable settings are reported and take Loupe's default.
pub fn linear_preferences(value: &Value) -> (ViewConfig, Vec<String>) {
    let mut config = ViewConfig {
        completed: CompletedWindow::All,
        ..ViewConfig::default()
    };
    let mut warnings = Vec::new();
    for (key, target) in [
        ("layout", 0),
        ("issueGrouping", 1),
        ("viewOrdering", 2),
        ("showCompletedIssues", 3),
    ] {
        let Some(text) = value.get(key).and_then(Value::as_str) else {
            continue;
        };
        let supported = match target {
            0 => match text {
                "board" => {
                    config.layout = Layout::Board;
                    true
                }
                "list" => {
                    config.layout = Layout::List;
                    true
                }
                _ => false,
            },
            1 => match text {
                "workflowState" | "status" => {
                    config.group_by = Grouping::Status;
                    true
                }
                "assignee" => {
                    config.group_by = Grouping::Assignee;
                    true
                }
                "priority" => {
                    config.group_by = Grouping::Priority;
                    true
                }
                "label" | "labels" => {
                    config.group_by = Grouping::Label;
                    true
                }
                "project" => {
                    config.group_by = Grouping::Project;
                    true
                }
                "cycle" => {
                    config.group_by = Grouping::Cycle;
                    true
                }
                "none" => {
                    config.group_by = Grouping::None;
                    true
                }
                _ => false,
            },
            2 => match text {
                "priority" => {
                    config.order_by = Ordering::Priority;
                    true
                }
                "updatedAt" => {
                    config.order_by = Ordering::Updated;
                    true
                }
                "createdAt" => {
                    config.order_by = Ordering::Created;
                    true
                }
                "dueDate" => {
                    config.order_by = Ordering::DueDate;
                    true
                }
                "manual" => {
                    config.order_by = Ordering::Manual;
                    true
                }
                _ => false,
            },
            _ => match text {
                "all" => true,
                "none" => {
                    config.completed = CompletedWindow::None;
                    true
                }
                "pastWeek" => {
                    config.completed = CompletedWindow::PastWeek;
                    true
                }
                "pastMonth" => {
                    config.completed = CompletedWindow::PastMonth;
                    true
                }
                _ => false,
            },
        };
        if !supported {
            warnings.push(format!("{key} ({text}) is not supported in Loupe."));
        }
    }
    config.show_empty_columns = value
        .get(empty_groups_key(config.layout))
        .and_then(Value::as_bool)
        .or_else(|| value.get("showEmptyGroups").and_then(Value::as_bool))
        .unwrap_or(false);
    config.hidden_groups = value
        .get(hidden_groups_key(config.layout))
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| group.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    config.column_order = value
        .get("columnOrderBoard")
        .and_then(Value::as_array)
        .map(|keys| {
            keys.iter()
                .filter_map(|key| key.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    for (field, property) in PROPERTY_FIELDS {
        match value.get(field).and_then(Value::as_bool) {
            Some(true) if !config.properties.contains(&property) => {
                config.properties.push(property)
            }
            Some(false) => config.properties.retain(|known| *known != property),
            _ => {}
        }
    }
    if let Some(direction) = value.get("viewOrderingDirection").and_then(Value::as_str) {
        config.sort_direction = match direction {
            "ascending" | "asc" => Some(SortDirection::Ascending),
            "descending" | "desc" => Some(SortDirection::Descending),
            _ => {
                warnings.push(format!(
                    "viewOrderingDirection ({direction}) is not supported in Loupe."
                ));
                None
            }
        };
    }
    config.show_sub_issues = value.get("showSubIssues").and_then(Value::as_bool) != Some(false);
    (config, warnings)
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, Utc};
    use serde_json::{Value, json};

    use super::{
        CompletedWindow, DisplayProperty, Grouping, ImportContext, Layout, Ordering, SortDirection,
        ViewConfig, export_linear_filter, export_preferences, import_linear_filter,
        linear_preferences,
    };
    use crate::filter::{
        CURRENT_CYCLE, DateField, DateOp, FilterClause, FlagField, ME, NONE, SetField, SetOp,
    };
    use crate::types::{Label, StateType, WorkflowState};

    fn label(id: &str, name: &str, parent: Option<&str>) -> Label {
        Label {
            id: id.into(),
            name: name.into(),
            color: "#888888".into(),
            parent_id: parent.map(Into::into),
        }
    }

    fn import(filter: serde_json::Value) -> Result<Vec<FilterClause>, String> {
        let labels = vec![
            label("g-area", "Area", None),
            label("l-api", "API", Some("g-area")),
            label("l-ui", "UI", Some("g-area")),
            label("l-bug", "Bug", None),
        ];
        let states = vec![WorkflowState {
            id: "s-review".into(),
            name: "In Review".into(),
            kind: StateType::Started,
            position: 1.0,
            color: "#000000".into(),
        }];
        import_linear_filter(
            &filter,
            &ImportContext {
                labels: &labels,
                states: &states,
            },
        )
    }

    #[test]
    fn an_or_of_one_field_becomes_one_set_clause() {
        let clauses = import(json!({ "or": [] }));
        assert!(clauses.is_err());
        let clauses = import(json!({ "priority": { "or": [{ "eq": 1 }, { "eq": 2 }] } }))
            .expect("priority or");
        assert_eq!(
            clauses,
            [FilterClause::Set {
                field: SetField::Priority,
                op: SetOp::In,
                values: vec!["1".into(), "2".into()],
            }]
        );
    }

    #[test]
    fn a_label_group_name_includes_its_labels() {
        let clauses = import(json!({ "labels": { "name": { "eq": "area" } } })).expect("labels");
        assert_eq!(
            clauses,
            [FilterClause::Set {
                field: SetField::Label,
                op: SetOp::In,
                values: vec!["g-area".into(), "l-api".into(), "l-ui".into()],
            }]
        );
    }

    #[test]
    /// Fields come out sorted by key, whatever serde_json's map order is.
    fn nulls_become_none_and_flags() {
        let clauses = import(json!({
            "assignee": { "null": true },
            "parent": { "null": false },
            "children": { "length": { "gt": 0 } },
        }))
        .expect("nulls");
        assert_eq!(
            clauses,
            [
                FilterClause::Set {
                    field: SetField::Assignee,
                    op: SetOp::In,
                    values: vec![NONE.into()],
                },
                FilterClause::Flag {
                    field: FlagField::HasSubIssues,
                    value: true,
                },
                FilterClause::Flag {
                    field: FlagField::IsSubIssue,
                    value: true,
                },
            ]
        );
    }

    #[test]
    fn what_cannot_be_reproduced_rejects_the_view() {
        assert!(import(json!({ "estimate": { "gt": 2 } })).is_err());
        assert!(import(json!({ "labels": { "name": { "eq": "Nope" } } })).is_err());
        assert!(
            import(json!({ "state": { "or": [
                { "and": [{ "type": { "eq": "started" } }, { "name": { "eq": "In Review" } }] },
                { "type": { "eq": "unstarted" } },
            ] } }))
            .is_err(),
            "an and inside an or would widen the view"
        );
        assert!(import(json!({ "assignee": { "isMe": { "eq": false } } })).is_err());
    }

    fn set(field: SetField, op: SetOp, values: &[&str]) -> FilterClause {
        FilterClause::Set {
            field,
            op,
            values: values.iter().map(|value| (*value).to_owned()).collect(),
        }
    }

    /// Every kind of clause Linear can keep exactly.
    fn exportable() -> Vec<FilterClause> {
        let day = NaiveDate::from_ymd_opt(2026, 11, 1).expect("a day");
        let mut clauses = vec![
            set(SetField::Status, SetOp::In, &["s-review", "s-done"]),
            set(SetField::Status, SetOp::NotIn, &["s-review"]),
            set(SetField::StatusType, SetOp::In, &["unstarted", "started"]),
            set(SetField::StatusType, SetOp::NotIn, &["canceled"]),
            set(SetField::Priority, SetOp::In, &["1", "2"]),
            set(SetField::Priority, SetOp::NotIn, &["0"]),
            set(SetField::Assignee, SetOp::In, &[ME, "u-1", NONE]),
            set(SetField::Assignee, SetOp::In, &["u-1"]),
            set(SetField::Assignee, SetOp::NotIn, &[NONE]),
            set(SetField::Creator, SetOp::In, &[ME]),
            set(SetField::Project, SetOp::In, &["p-1", NONE]),
            set(SetField::Milestone, SetOp::In, &["m-1"]),
            set(SetField::Cycle, SetOp::In, &[CURRENT_CYCLE, "c-1", NONE]),
            set(SetField::Label, SetOp::In, &["l-bug", NONE, "l-api"]),
        ];
        for field in [
            DateField::CreatedAt,
            DateField::UpdatedAt,
            DateField::DueDate,
        ] {
            for op in [DateOp::Before(day), DateOp::After(day)] {
                clauses.push(FilterClause::Date { field, op });
            }
        }
        for field in [
            FlagField::IsSubIssue,
            FlagField::HasSubIssues,
            FlagField::HasDueDate,
        ] {
            for value in [true, false] {
                clauses.push(FilterClause::Flag { field, value });
            }
        }
        clauses
    }

    #[test]
    fn a_filter_saved_in_linear_reads_back_as_the_same_clauses() {
        let clauses = exportable();
        let filter = export_linear_filter(&clauses).expect("every clause is exportable");
        assert_eq!(import(filter).expect("the export imports"), clauses);
        // One clause at a time too: nothing leans on its neighbors.
        for clause in clauses {
            let alone = std::slice::from_ref(&clause);
            let filter = export_linear_filter(alone).expect("exportable");
            assert_eq!(import(filter).as_deref(), Ok(alone), "{clause:?}");
        }
        assert_eq!(export_linear_filter(&[]), Ok(json!({})));
        assert_eq!(import(json!({})), Ok(Vec::new()));
    }

    #[test]
    fn a_filter_linear_cannot_keep_exactly_is_refused_by_name() {
        let refused = [
            (set(SetField::Assignee, SetOp::NotIn, &["u-1"]), "Assignee"),
            (set(SetField::Label, SetOp::NotIn, &["l-bug"]), "Label"),
            (
                set(SetField::PullRequest, SetOp::In, &["open"]),
                "Pull request",
            ),
            (set(SetField::Project, SetOp::In, &[ME]), "Project"),
            (set(SetField::Priority, SetOp::In, &["high"]), "Priority"),
            (
                FilterClause::Date {
                    field: DateField::UpdatedAt,
                    op: DateOp::PastDays(7),
                },
                "Updated",
            ),
            (
                FilterClause::Date {
                    field: DateField::ClosedAt,
                    op: DateOp::IsSet,
                },
                "Closed",
            ),
            (
                FilterClause::Flag {
                    field: FlagField::HasPullRequests,
                    value: true,
                },
                "Has pull requests",
            ),
            (
                FilterClause::Text {
                    value: "login".into(),
                },
                "Search text",
            ),
        ];
        for (clause, name) in refused {
            let mut clauses = exportable();
            clauses.push(clause.clone());
            let error = export_linear_filter(&clauses).expect_err("refused");
            assert!(error.contains(name), "{clause:?}: {error}");
        }
    }

    #[test]
    fn display_settings_saved_in_linear_read_back_the_same() {
        let configs = [
            ViewConfig::default(),
            ViewConfig {
                layout: Layout::List,
                group_by: Grouping::Assignee,
                order_by: Ordering::Manual,
                sort_direction: Some(SortDirection::Descending),
                completed: CompletedWindow::None,
                show_empty_columns: false,
                hidden_groups: vec!["s-review".into()],
                column_order: vec!["s-review".into(), "s-todo".into()],
                show_sub_issues: false,
                properties: vec![DisplayProperty::Id, DisplayProperty::Status],
                filters: Vec::new(),
            },
            ViewConfig {
                group_by: Grouping::None,
                order_by: Ordering::DueDate,
                sort_direction: Some(SortDirection::Ascending),
                completed: CompletedWindow::All,
                hidden_groups: vec!["a".into(), "b".into()],
                column_order: vec!["b".into(), "c".into(), "a".into()],
                ..ViewConfig::default()
            },
        ];
        for config in &configs {
            let written = export_preferences(config, None, &Value::Null);
            let (read, warnings) = linear_preferences(&written);
            assert_eq!(&read, config);
            assert_eq!(warnings, Vec::<String>::new());
        }
        // A change over a saved view: written over what the owner had,
        // the result reads back as the new settings too.
        for saved in &configs {
            for config in &configs {
                let own = export_preferences(saved, None, &Value::Null);
                let written = export_preferences(config, Some(saved), &own);
                assert_eq!(&linear_preferences(&written).0, config);
            }
        }
    }

    #[test]
    fn a_save_writes_only_the_display_settings_the_user_changed() {
        // The owner's view in Linear: grouped by something Loupe does not
        // have, empty groups hidden on the board and shown on the list.
        let own = json!({
            "layout": "board",
            "issueGrouping": "team",
            "showEmptyGroupsBoard": false,
            "showEmptyGroupsList": true,
        });
        let (saved, warnings) = linear_preferences(&own);
        assert_eq!(saved.group_by, Grouping::Status, "read as Loupe's default");
        assert_eq!(warnings.len(), 1);
        assert!(!saved.show_empty_columns);

        // The user changes the ordering and nothing else.
        let draft = ViewConfig {
            order_by: Ordering::Updated,
            ..saved.clone()
        };
        let written = export_preferences(&draft, Some(&saved), &own);
        assert_eq!(
            written,
            json!({
                "layout": "board",
                "issueGrouping": "team",
                "showEmptyGroupsBoard": false,
                "showEmptyGroupsList": true,
                "viewOrdering": "updatedAt",
            })
        );

        // Nothing changed: nothing written but what the owner had.
        assert_eq!(export_preferences(&saved, Some(&saved), &own), own);
    }

    #[test]
    fn saving_display_settings_keeps_what_the_owner_had_set_in_linear() {
        let own = json!({ "issueSubGrouping": "priority", "layout": "list", "fieldSla": true });
        let written = export_preferences(&ViewConfig::default(), None, &own);
        assert_eq!(written["issueSubGrouping"], "priority");
        assert_eq!(written["fieldSla"], true);
        assert_eq!(written["layout"], "board", "Loupe's own settings win");
    }

    #[test]
    fn hidden_sub_issues_are_a_display_setting_that_narrows_the_query() {
        let (config, _) = linear_preferences(&json!({ "showSubIssues": false }));
        assert!(config.filters.is_empty());
        assert!(!config.show_sub_issues);
        let now = Utc::now();
        assert_eq!(
            config.query(now).clauses,
            [FilterClause::Flag {
                field: FlagField::IsSubIssue,
                value: false,
            }]
        );
        assert!(ViewConfig::default().query(now).clauses.is_empty());
    }
}
