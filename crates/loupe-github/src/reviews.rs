//! The Reviews screen's filters and saved views. A view keeps the pull
//! requests that match every clause; inside one clause, values are
//! alternatives ("Review is Approved, Changes requested"). Filtering runs on
//! the pull requests the inbox already loaded, so the tabs and their counts
//! always agree with what the screen lists. A view that names repositories
//! can also list every open pull request in them (the All tab). Inside a
//! tab, pull requests group by their next step ([`next_step`]). Views live
//! in a JSON file the app owns, never on GitHub.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{ChecksState, Mergeable, PrState, PrSummary, ReviewDecision};

/// The file format version [`save`] writes.
pub const VIEWS_VERSION: u32 = 1;
pub const MAX_NAME: usize = 60;

/// What a clause looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Field {
    /// The repository owner's login.
    Org,
    /// `owner/name`.
    Repo,
    Author,
    Assignee,
    /// Users asked to review or who reviewed.
    Reviewer,
    /// Label names.
    Label,
    /// [`OPEN`] (open, not a draft) or [`DRAFT`].
    Status,
    /// `approved`, `changes_requested`, `review_required`.
    Review,
    /// `passing`, `failing`, `pending`.
    Checks,
    /// `mergeable`, `conflicting`.
    Merge,
}

pub const OPEN: &str = "open";
pub const DRAFT: &str = "draft";

impl Field {
    pub const ALL: [Field; 10] = [
        Field::Org,
        Field::Repo,
        Field::Author,
        Field::Assignee,
        Field::Reviewer,
        Field::Label,
        Field::Status,
        Field::Review,
        Field::Checks,
        Field::Merge,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Field::Org => "Organization",
            Field::Repo => "Repository",
            Field::Author => "Author",
            Field::Assignee => "Assignee",
            Field::Reviewer => "Reviewer",
            Field::Label => "Label",
            Field::Status => "Status",
            Field::Review => "Review",
            Field::Checks => "Checks",
            Field::Merge => "Merge",
        }
    }

    /// The values of a field whose values are fixed, as `(value, label)`;
    /// empty for fields whose values come from the loaded pull requests.
    pub fn fixed_values(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Field::Status => &[(OPEN, "Open"), (DRAFT, "Draft")],
            Field::Review => &[
                ("approved", "Approved"),
                ("changes_requested", "Changes requested"),
                ("review_required", "Waiting for review"),
            ],
            Field::Checks => &[
                ("passing", "Checks passing"),
                ("failing", "Checks failing"),
                ("pending", "Checks pending"),
            ],
            Field::Merge => &[("mergeable", "Mergeable"), ("conflicting", "Has conflicts")],
            _ => &[],
        }
    }
}

/// `In`: is / is any of. `NotIn`: is not / is none of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Op {
    In,
    NotIn,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Clause {
    pub field: Field,
    pub op: Op,
    pub values: Vec<String>,
}

/// The values `pr` has for `field`, in the same spelling the options use.
fn values_of(pr: &PrSummary, field: Field) -> Vec<String> {
    let logins = |actors: &[crate::Actor]| -> Vec<String> {
        actors.iter().map(|a| a.login.clone()).collect()
    };
    let one =
        |value: Option<&str>| -> Vec<String> { value.map(str::to_owned).into_iter().collect() };
    match field {
        Field::Org => vec![pr.pr.repo.owner.clone()],
        Field::Repo => vec![pr.pr.repo.to_string()],
        Field::Author => one(pr.author.as_ref().map(|author| author.login.as_str())),
        Field::Assignee => logins(&pr.assignees),
        Field::Reviewer => logins(&pr.reviewers),
        Field::Label => pr.labels.iter().map(|label| label.name.clone()).collect(),
        Field::Status => one(match (pr.state, pr.is_draft) {
            (PrState::Open, false) => Some(OPEN),
            (PrState::Open, true) => Some(DRAFT),
            _ => None,
        }),
        Field::Review => one(pr.review_decision.map(|decision| match decision {
            ReviewDecision::Approved => "approved",
            ReviewDecision::ChangesRequested => "changes_requested",
            ReviewDecision::ReviewRequired => "review_required",
        })),
        Field::Checks => one(pr.checks.map(|checks| match checks {
            ChecksState::Passing => "passing",
            ChecksState::Failing => "failing",
            ChecksState::Pending => "pending",
        })),
        Field::Merge => one(match pr.mergeable {
            Mergeable::Mergeable => Some("mergeable"),
            Mergeable::Conflicting => Some("conflicting"),
            Mergeable::Unknown => None,
        }),
    }
}

/// Whether `pr` passes every clause. Logins, owners, and label names compare
/// without case, as GitHub's search does.
pub fn matches(pr: &PrSummary, clauses: &[Clause]) -> bool {
    clauses.iter().all(|clause| {
        let has = values_of(pr, clause.field);
        let any = clause
            .values
            .iter()
            .any(|wanted| has.iter().any(|value| value.eq_ignore_ascii_case(wanted)));
        match clause.op {
            Op::In => any,
            Op::NotIn => !any,
        }
    })
}

/// One value a field can take, as the filter menu lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldOption {
    pub value: String,
    pub label: String,
}

/// The values `field` offers: the fixed ones, or the distinct values of the
/// loaded `prs`, sorted without case. Repositories narrow to the
/// organizations an "is" organization clause picks. Values the clauses hold
/// that no loaded pull request has stay listed, so a saved view can still be
/// edited.
pub fn options<'a>(
    field: Field,
    prs: impl IntoIterator<Item = &'a PrSummary>,
    clauses: &[Clause],
) -> Vec<FieldOption> {
    let fixed = field.fixed_values();
    if !fixed.is_empty() {
        return fixed
            .iter()
            .map(|(value, label)| FieldOption {
                value: (*value).to_owned(),
                label: (*label).to_owned(),
            })
            .collect();
    }
    let orgs: Vec<&String> = clauses
        .iter()
        .filter(|clause| field == Field::Repo && clause.field == Field::Org && clause.op == Op::In)
        .flat_map(|clause| clause.values.iter())
        .collect();
    let in_orgs = |repo: &str| {
        orgs.is_empty()
            || orgs.iter().any(|org| {
                repo.split_once('/')
                    .is_some_and(|(owner, _)| owner.eq_ignore_ascii_case(org))
            })
    };
    fn add(values: &mut Vec<String>, value: String) {
        if !values
            .iter()
            .any(|known| known.eq_ignore_ascii_case(&value))
        {
            values.push(value);
        }
    }
    let mut values: Vec<String> = Vec::new();
    for pr in prs {
        for value in values_of(pr, field) {
            if field != Field::Repo || in_orgs(&value) {
                add(&mut values, value);
            }
        }
    }
    values.sort_by_key(|value| value.to_lowercase());
    for value in selected(clauses, field) {
        add(&mut values, value);
    }
    values
        .into_iter()
        .map(|value| FieldOption {
            label: value.clone(),
            value,
        })
        .collect()
}

/// The values the `field` clause holds now.
pub fn selected(clauses: &[Clause], field: Field) -> Vec<String> {
    clauses
        .iter()
        .find(|clause| clause.field == field)
        .map(|clause| clause.values.clone())
        .unwrap_or_default()
}

/// Adds `value` to the `field` clause (creating an "is" clause), or removes
/// it; a clause left without values goes away.
pub fn toggle_value(clauses: &mut Vec<Clause>, field: Field, value: &str) {
    match clauses.iter().position(|clause| clause.field == field) {
        Some(index) => {
            let values = &mut clauses[index].values;
            match values.iter().position(|known| known == value) {
                Some(at) => {
                    values.remove(at);
                }
                None => values.push(value.to_owned()),
            }
            if values.is_empty() {
                clauses.remove(index);
            }
        }
        None => clauses.push(Clause {
            field,
            op: Op::In,
            values: vec![value.to_owned()],
        }),
    }
}

/// Sets the operator of the clause at `index`.
pub fn set_op(clauses: &mut [Clause], index: usize, op: Op) {
    if let Some(clause) = clauses.get_mut(index) {
        clause.op = op;
    }
}

/// How a clause reads on its chip, such as "Review is Approved" or
/// "Repository is none of a/b, a/c".
pub fn describe(clause: &Clause) -> String {
    let fixed = clause.field.fixed_values();
    let names: Vec<&str> = clause
        .values
        .iter()
        .map(|value| {
            fixed
                .iter()
                .find(|(known, _)| known == value)
                .map_or(value.as_str(), |(_, label)| label)
        })
        .collect();
    let verb = match (clause.op, names.len()) {
        (Op::In, 1) => "is",
        (Op::In, _) => "is any of",
        (Op::NotIn, 1) => "is not",
        (Op::NotIn, _) => "is none of",
    };
    let shown = if names.len() > 2 {
        format!("{}, {} +{}", names[0], names[1], names.len() - 2)
    } else {
        names.join(", ")
    };
    format!("{} {verb} {shown}", clause.field.label())
}

/// The search behind a view's "All open" section: every open pull request
/// in the repositories its "is" repository clause names, or else in the
/// organizations its "is" organization clause names, most recently updated
/// first. `None` when the view names neither, since the section would then
/// cover all of GitHub. Only the scope goes to GitHub; the other clauses
/// filter the result like every other section, so editing them never
/// starts another search.
pub fn all_open_query(clauses: &[Clause]) -> Option<String> {
    let scope = |field: Field, qualifier: &str| -> Option<String> {
        let values: Vec<String> = clauses
            .iter()
            .filter(|clause| clause.field == field && clause.op == Op::In)
            .flat_map(|clause| clause.values.iter())
            .map(|value| format!("{qualifier}:{value}"))
            .collect();
        (!values.is_empty()).then(|| values.join(" "))
    };
    let scope = scope(Field::Repo, "repo").or_else(|| scope(Field::Org, "org"))?;
    Some(format!(
        "is:pr is:open archived:false {scope} sort:updated-desc"
    ))
}

/// One tab of the Reviews screen, in the order the tab row shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewTab {
    /// `review-requested:@me`.
    ToReview,
    /// `author:@me`.
    Mine,
    /// `involves:@me`, minus the two above.
    Involved,
    /// Every pull request the other tabs list, and on a view that names
    /// repositories every other open one in them.
    All,
}

impl ReviewTab {
    pub const ALL: [ReviewTab; 4] = [
        ReviewTab::ToReview,
        ReviewTab::Mine,
        ReviewTab::Involved,
        ReviewTab::All,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ReviewTab::ToReview => "To review",
            ReviewTab::Mine => "Mine",
            ReviewTab::Involved => "Involved",
            ReviewTab::All => "All",
        }
    }
}

/// What a pull request waits on next: the groups inside a tab, in the order
/// they are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NextStep {
    ReadyToMerge,
    ChecksFailing,
    ChangesRequested,
    WaitingForReview,
    Draft,
}

impl NextStep {
    pub const ALL: [NextStep; 5] = [
        NextStep::ReadyToMerge,
        NextStep::ChecksFailing,
        NextStep::ChangesRequested,
        NextStep::WaitingForReview,
        NextStep::Draft,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NextStep::ReadyToMerge => "Ready to merge",
            NextStep::ChecksFailing => "Checks failing",
            NextStep::ChangesRequested => "Changes requested",
            NextStep::WaitingForReview => "Waiting for review",
            NextStep::Draft => "Draft",
        }
    }
}

/// The one group `pr` belongs to: the first of [`NextStep::ALL`] whose rule
/// it meets.
///
/// 1. Ready to merge: not a draft, approved, no check failing or still
///    running (a pull request without checks has none to wait for), and no
///    conflict (`Unknown`, not computed yet, is not a conflict).
/// 2. Checks failing.
/// 3. Changes requested.
/// 4. Waiting for review: every other pull request that is not a draft,
///    including an approved one whose checks still run or that conflicts.
/// 5. Draft.
pub fn next_step(pr: &PrSummary) -> NextStep {
    let checks_green = !matches!(pr.checks, Some(ChecksState::Failing | ChecksState::Pending));
    if !pr.is_draft
        && pr.review_decision == Some(ReviewDecision::Approved)
        && checks_green
        && pr.mergeable != Mergeable::Conflicting
    {
        NextStep::ReadyToMerge
    } else if pr.checks == Some(ChecksState::Failing) {
        NextStep::ChecksFailing
    } else if pr.review_decision == Some(ReviewDecision::ChangesRequested) {
        NextStep::ChangesRequested
    } else if !pr.is_draft {
        NextStep::WaitingForReview
    } else {
        NextStep::Draft
    }
}

/// What a section's rows can be sorted by: the row's lanes, with its age
/// read from either date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortKey {
    Updated,
    Created,
    /// Lines added plus lines removed.
    Changes,
    Comments,
    Checks,
    Author,
    Repository,
}

impl SortKey {
    pub const ALL: [SortKey; 7] = [
        SortKey::Updated,
        SortKey::Created,
        SortKey::Changes,
        SortKey::Comments,
        SortKey::Checks,
        SortKey::Author,
        SortKey::Repository,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Updated => "Updated",
            SortKey::Created => "Created",
            SortKey::Changes => "Changes",
            SortKey::Comments => "Comments",
            SortKey::Checks => "Checks",
            SortKey::Author => "Author",
            SortKey::Repository => "Repository",
        }
    }

    /// How each direction reads for this key, ascending first.
    pub fn directions(self) -> (&'static str, &'static str) {
        match self {
            SortKey::Updated | SortKey::Created => ("Oldest first", "Newest first"),
            SortKey::Changes => ("Smallest first", "Largest first"),
            SortKey::Comments => ("Fewest first", "Most first"),
            SortKey::Checks => ("Failing first", "Passing first"),
            SortKey::Author | SortKey::Repository => ("A to Z", "Z to A"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortDir {
    Ascending,
    Descending,
}

/// A sort order for each section's rows. No sort (`None` wherever an
/// `Option<Sort>` is kept) is GitHub's own order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Sort {
    pub key: SortKey,
    pub dir: SortDir,
}

impl Sort {
    /// How this direction reads, such as "Largest first".
    pub fn describe(self) -> &'static str {
        let (ascending, descending) = self.key.directions();
        match self.dir {
            SortDir::Ascending => ascending,
            SortDir::Descending => descending,
        }
    }
}

/// The sort after a click on `key`'s column heading: ascending, then
/// descending, then back to the default order. A click on another column
/// starts it at ascending.
pub fn cycle_sort(current: Option<Sort>, key: SortKey) -> Option<Sort> {
    let dir = match current {
        Some(sort) if sort.key == key => match sort.dir {
            SortDir::Ascending => SortDir::Descending,
            SortDir::Descending => return None,
        },
        _ => SortDir::Ascending,
    };
    Some(Sort { key, dir })
}

/// The value `pr` sorts by, or `None` when it has none for `key`.
fn sort_value(pr: &PrSummary, key: SortKey) -> Option<SortValue> {
    Some(match key {
        SortKey::Updated => SortValue::Number(pr.updated_at.timestamp_millis()),
        SortKey::Created => SortValue::Number(pr.created_at.timestamp_millis()),
        SortKey::Changes => SortValue::Number(i64::from(pr.additions) + i64::from(pr.deletions)),
        SortKey::Comments => SortValue::Number(i64::from(pr.comment_count)),
        // Failing ranks lowest, so ascending puts what needs attention first.
        SortKey::Checks => SortValue::Number(match pr.checks? {
            ChecksState::Failing => 0,
            ChecksState::Pending => 1,
            ChecksState::Passing => 2,
        }),
        SortKey::Author => SortValue::Text(pr.author.as_ref()?.login.to_lowercase()),
        SortKey::Repository => SortValue::Text(pr.pr.repo.to_string().to_lowercase()),
    })
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum SortValue {
    Number(i64),
    Text(String),
}

/// Sorts `rows` in place. The sort is stable, so rows that tie keep the
/// order they came in; rows with no value for the key (no checks, a deleted
/// author) go last in either direction. `None` leaves the order as it is.
pub fn sort_rows<T: std::borrow::Borrow<PrSummary>>(rows: &mut [T], sort: Option<Sort>) {
    let Some(sort) = sort else {
        return;
    };
    rows.sort_by(|a, b| {
        let (a, b) = (
            sort_value(a.borrow(), sort.key),
            sort_value(b.borrow(), sort.key),
        );
        match (a, b) {
            (Some(a), Some(b)) => match sort.dir {
                SortDir::Ascending => a.cmp(&b),
                SortDir::Descending => b.cmp(&a),
            },
            (a, b) => a.is_none().cmp(&b.is_none()),
        }
    });
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedView {
    pub id: String,
    pub name: String,
    pub filters: Vec<Clause>,
    /// Whether the view shows its "All open" section. `None` (the view was
    /// never switched) means on when the view names repositories or
    /// organizations: see [`SavedView::shows_all_open`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_open: Option<bool>,
    /// The order of each section's rows; `None` is GitHub's order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<Sort>,
    /// The tab the view opens on; `None` opens the first tab with pull
    /// requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<ReviewTab>,
    /// The groups folded in every tab.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collapsed: Vec<NextStep>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SavedView {
    /// Whether the "All open" section is on: the saved choice, or on by
    /// default for a view that names repositories or organizations.
    pub fn shows_all_open(&self) -> bool {
        self.all_open
            .unwrap_or_else(|| all_open_query(&self.filters).is_some())
    }
}

/// The saved views, in tab order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewsFile {
    pub version: u32,
    pub views: Vec<SavedView>,
}

fn clean_name(name: &str) -> String {
    let name: String = name.trim().chars().take(MAX_NAME).collect();
    if name.is_empty() {
        "Untitled view".to_owned()
    } else {
        name
    }
}

impl ViewsFile {
    pub fn get(&self, id: &str) -> Option<&SavedView> {
        self.views.iter().find(|view| view.id == id)
    }

    /// Adds a view at the end and returns its id. The name is trimmed and
    /// cut to [`MAX_NAME`] characters; a blank name becomes "Untitled view".
    pub fn create(&mut self, name: &str, filters: Vec<Clause>, now: DateTime<Utc>) -> String {
        let base = now.timestamp_nanos_opt().unwrap_or_default() as u64;
        let id = (0u64..)
            .map(|n| format!("rv_{:x}", base.wrapping_add(n)))
            .find(|id| self.get(id).is_none())
            .unwrap_or_default();
        self.views.push(SavedView {
            id: id.clone(),
            name: clean_name(name),
            filters,
            all_open: None,
            sort: None,
            tab: None,
            collapsed: Vec::new(),
            created_at: now,
            updated_at: now,
        });
        id
    }

    /// Whether the view exists.
    pub fn rename(&mut self, id: &str, name: &str, now: DateTime<Utc>) -> bool {
        self.edit(id, now, |view| view.name = clean_name(name))
    }

    pub fn update_filters(&mut self, id: &str, filters: Vec<Clause>, now: DateTime<Utc>) -> bool {
        self.edit(id, now, |view| view.filters = filters)
    }

    /// Turns the view's "All open" section on or off; whether the view exists.
    pub fn set_all_open(&mut self, id: &str, on: bool, now: DateTime<Utc>) -> bool {
        self.edit(id, now, |view| view.all_open = Some(on))
    }

    /// Sets the order of the view's rows; whether the view exists.
    pub fn set_sort(&mut self, id: &str, sort: Option<Sort>, now: DateTime<Utc>) -> bool {
        self.edit(id, now, |view| view.sort = sort)
    }

    /// Sets the tab the view opens on; whether the view exists.
    pub fn set_tab(&mut self, id: &str, tab: Option<ReviewTab>, now: DateTime<Utc>) -> bool {
        self.edit(id, now, |view| view.tab = tab)
    }

    /// Sets the groups the view keeps folded; whether the view exists.
    pub fn set_collapsed(
        &mut self,
        id: &str,
        collapsed: Vec<NextStep>,
        now: DateTime<Utc>,
    ) -> bool {
        self.edit(id, now, |view| view.collapsed = collapsed)
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.views.len();
        self.views.retain(|view| view.id != id);
        self.views.len() != before
    }

    fn edit(&mut self, id: &str, now: DateTime<Utc>, change: impl FnOnce(&mut SavedView)) -> bool {
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

pub enum Loaded {
    Read(ViewsFile),
    /// No file yet.
    Missing,
    /// The file could not be used; it was copied to `<file>.bak` so the next
    /// save does not lose it.
    Recovered {
        reason: String,
    },
}

/// Reads the views file at `path`, tolerating a missing, corrupt, or newer
/// file.
pub fn load(path: &Path) -> Loaded {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Loaded::Missing,
        Err(error) => {
            return Loaded::Recovered {
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
    Loaded::Recovered { reason }
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

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{
        Clause, Field, Loaded, NextStep, Op, ReviewTab, Sort, SortDir, SortKey, ViewsFile,
        all_open_query, cycle_sort, describe, load, matches, next_step, options, save, set_op,
        sort_rows, toggle_value,
    };
    use crate::{
        Actor, ChecksState, Label, Mergeable, PrRef, PrState, PrSummary, RepoRef, ReviewDecision,
    };

    fn actor(login: &str) -> Actor {
        Actor {
            login: login.into(),
            avatar_url: String::new(),
        }
    }

    fn pr(owner: &str, repo: &str, number: u64) -> PrSummary {
        let at = Utc::now();
        PrSummary {
            id: format!("PR_{owner}_{repo}_{number}"),
            pr: PrRef::new(RepoRef::new(owner, repo), number),
            title: format!("PR {number}"),
            url: String::new(),
            state: PrState::Open,
            is_draft: false,
            author: Some(actor("ana")),
            created_at: at,
            updated_at: at,
            comment_count: 0,
            additions: 0,
            deletions: 0,
            checks: None,
            review_decision: None,
            head_ref: "b".into(),
            labels: Vec::new(),
            assignees: Vec::new(),
            reviewers: Vec::new(),
            mergeable: Mergeable::Unknown,
        }
    }

    fn clause(field: Field, op: Op, values: &[&str]) -> Clause {
        Clause {
            field,
            op,
            values: values.iter().map(|value| (*value).to_owned()).collect(),
        }
    }

    fn numbers(prs: &[PrSummary], clauses: &[Clause]) -> Vec<u64> {
        prs.iter()
            .filter(|pr| matches(pr, clauses))
            .map(|pr| pr.pr.number)
            .collect()
    }

    fn sample() -> Vec<PrSummary> {
        let mut approved = pr("hooli-inc", "hooli-api", 1);
        approved.review_decision = Some(ReviewDecision::Approved);
        approved.checks = Some(ChecksState::Passing);
        approved.mergeable = Mergeable::Mergeable;
        approved.labels = vec![Label {
            id: "L1".into(),
            name: "revisao:risco".into(),
            color: "fff".into(),
        }];
        let mut draft = pr("hooli-inc", "hooli-infra", 2);
        draft.is_draft = true;
        draft.review_decision = Some(ReviewDecision::ChangesRequested);
        draft.checks = Some(ChecksState::Failing);
        draft.mergeable = Mergeable::Conflicting;
        draft.assignees = vec![actor("bo")];
        let mut waiting = pr("acme", "web", 3);
        waiting.review_decision = Some(ReviewDecision::ReviewRequired);
        waiting.checks = Some(ChecksState::Pending);
        waiting.author = Some(actor("cy"));
        waiting.reviewers = vec![actor("IsraelAraujo70")];
        vec![approved, draft, waiting]
    }

    #[test]
    fn every_clause_must_match_and_values_are_alternatives() {
        let prs = sample();
        let org = clause(Field::Org, Op::In, &["hooli-inc"]);
        assert_eq!(numbers(&prs, std::slice::from_ref(&org)), [1, 2]);
        let repo = clause(Field::Repo, Op::In, &["hooli-inc/hooli-api"]);
        assert_eq!(numbers(&prs, &[org.clone(), repo.clone()]), [1]);
        let approved = clause(Field::Review, Op::In, &["approved"]);
        assert_eq!(numbers(&prs, &[org.clone(), repo, approved]), [1]);
        let either = clause(Field::Review, Op::In, &["approved", "review_required"]);
        assert_eq!(numbers(&prs, &[either]), [1, 3]);
        let not_mine = clause(Field::Author, Op::NotIn, &["ana"]);
        assert_eq!(numbers(&prs, &[not_mine]), [3]);
        assert_eq!(numbers(&prs, &[]), [1, 2, 3]);
    }

    #[test]
    fn status_fields_follow_the_pull_request_state() {
        let prs = sample();
        let by = |field, value| numbers(&prs, &[clause(field, Op::In, &[value])]);
        assert_eq!(by(Field::Status, "open"), [1, 3], "drafts are not open");
        assert_eq!(by(Field::Status, "draft"), [2]);
        assert_eq!(by(Field::Review, "changes_requested"), [2]);
        assert_eq!(by(Field::Review, "review_required"), [3]);
        assert_eq!(by(Field::Checks, "passing"), [1]);
        assert_eq!(by(Field::Checks, "failing"), [2]);
        assert_eq!(by(Field::Checks, "pending"), [3]);
        assert_eq!(by(Field::Merge, "mergeable"), [1]);
        assert_eq!(by(Field::Merge, "conflicting"), [2]);
        assert_eq!(by(Field::Assignee, "bo"), [2]);
        assert_eq!(by(Field::Reviewer, "israelaraujo70"), [3]);
        assert_eq!(by(Field::Label, "revisao:risco"), [1]);
    }

    #[test]
    fn repositories_narrow_to_the_chosen_organization() {
        let prs = sample();
        let values = |clauses: &[Clause]| -> Vec<String> {
            options(Field::Repo, &prs, clauses)
                .into_iter()
                .map(|option| option.value)
                .collect()
        };
        assert_eq!(
            values(&[]),
            ["acme/web", "hooli-inc/hooli-api", "hooli-inc/hooli-infra"]
        );
        let org = clause(Field::Org, Op::In, &["hooli-inc"]);
        assert_eq!(
            values(std::slice::from_ref(&org)),
            ["hooli-inc/hooli-api", "hooli-inc/hooli-infra"]
        );
        let excluded = clause(Field::Org, Op::NotIn, &["hooli-inc"]);
        assert_eq!(values(&[excluded]).len(), 3, "only an is clause narrows");
        let saved = clause(Field::Repo, Op::In, &["gone/repo"]);
        assert!(values(&[org, saved]).contains(&"gone/repo".to_owned()));
    }

    #[test]
    fn people_and_orgs_come_from_the_loaded_pull_requests() {
        let prs = sample();
        let values = |field| -> Vec<String> {
            options(field, &prs, &[])
                .into_iter()
                .map(|option| option.value)
                .collect()
        };
        assert_eq!(values(Field::Org), ["acme", "hooli-inc"]);
        assert_eq!(values(Field::Author), ["ana", "cy"]);
        assert_eq!(values(Field::Reviewer), ["IsraelAraujo70"]);
        assert_eq!(values(Field::Review).len(), 3);
    }

    #[test]
    fn chips_read_like_sentences_and_toggling_builds_clauses() {
        let mut clauses = Vec::new();
        toggle_value(&mut clauses, Field::Review, "approved");
        assert_eq!(describe(&clauses[0]), "Review is Approved");
        toggle_value(&mut clauses, Field::Review, "review_required");
        toggle_value(&mut clauses, Field::Review, "changes_requested");
        assert_eq!(
            describe(&clauses[0]),
            "Review is any of Approved, Waiting for review +1"
        );
        set_op(&mut clauses, 0, Op::NotIn);
        assert!(describe(&clauses[0]).starts_with("Review is none of"));
        for value in ["approved", "review_required", "changes_requested"] {
            toggle_value(&mut clauses, Field::Review, value);
        }
        assert!(clauses.is_empty());
    }

    #[test]
    fn all_open_searches_the_view_repositories_or_else_its_organizations() {
        let repos = clause(
            Field::Repo,
            Op::In,
            &["hooli-inc/hooli-api", "hooli-inc/hooli-infra"],
        );
        let org = clause(Field::Org, Op::In, &["hooli-inc"]);
        let approved = clause(Field::Review, Op::In, &["approved"]);
        assert_eq!(
            all_open_query(&[org.clone(), repos.clone(), approved.clone()]).as_deref(),
            Some(
                "is:pr is:open archived:false repo:hooli-inc/hooli-api \
                 repo:hooli-inc/hooli-infra sort:updated-desc"
            ),
            "repositories narrow more than their organization"
        );
        assert_eq!(
            all_open_query(&[org, approved.clone()]).as_deref(),
            Some("is:pr is:open archived:false org:hooli-inc sort:updated-desc")
        );
        let excluded = clause(Field::Repo, Op::NotIn, &["a/b"]);
        assert_eq!(all_open_query(&[excluded, approved]), None);
        assert_eq!(all_open_query(&[]), None);
    }

    #[test]
    fn all_open_is_on_by_default_only_where_it_has_repositories() {
        let now = Utc::now();
        let mut file = ViewsFile::default();
        let repo = file.create(
            "Back end",
            vec![clause(Field::Repo, Op::In, &["hooli-inc/hooli-api"])],
            now,
        );
        let mine = file.create(
            "Mine",
            vec![clause(Field::Author, Op::In, &["IsraelAraujo70"])],
            now,
        );
        assert!(file.get(&repo).unwrap().shows_all_open());
        assert!(!file.get(&mine).unwrap().shows_all_open());
        assert!(file.set_all_open(&repo, false, now));
        assert!(file.set_all_open(&mine, true, now));
        assert!(!file.get(&repo).unwrap().shows_all_open());
        assert!(file.get(&mine).unwrap().shows_all_open());
        assert!(!file.set_all_open("missing", true, now));

        // A file written before the switch existed reads as the default.
        let old = r#"{"version":1,"views":[{"id":"rv_1","name":"Back end Hooli",
            "filters":[{"field":"repo","op":"in","values":["hooli-inc/hooli-api"]}],
            "createdAt":"2026-11-11T00:00:00Z","updatedAt":"2026-11-11T00:00:00Z"}]}"#;
        let old: ViewsFile = serde_json::from_str(old).unwrap();
        assert_eq!(old.views[0].all_open, None);
        assert_eq!(
            old.views[0].sort, None,
            "a view saved before sorting keeps GitHub's order"
        );
        assert!(old.views[0].shows_all_open());
    }

    fn sorted(prs: &[PrSummary], key: SortKey, dir: SortDir) -> Vec<u64> {
        let mut rows: Vec<&PrSummary> = prs.iter().collect();
        sort_rows(&mut rows, Some(Sort { key, dir }));
        rows.iter().map(|pr| pr.pr.number).collect()
    }

    /// Five pull requests in GitHub's order (1 to 5), each field set so
    /// that every key orders them differently.
    fn sortable() -> Vec<PrSummary> {
        let day = |n: i64| Utc::now() - chrono::Duration::days(n);
        #[rustfmt::skip]
        let rows = [
            // number, repo, author, created, updated, changes, comments, checks
            (1, "web", Some("dora"), 3, 1, (10, 5), 4, Some(ChecksState::Passing)),
            (2, "api", Some("ana"), 9, 2, (300, 0), 0, Some(ChecksState::Failing)),
            (3, "infra", None, 1, 5, (0, 2), 9, None),
            (4, "api", Some("Cy"), 6, 3, (40, 40), 1, Some(ChecksState::Pending)),
            (5, "zeta", Some("bo"), 4, 4, (1, 2), 4, Some(ChecksState::Failing)),
        ];
        rows.into_iter()
            .map(
                |(number, repo, author, created, updated, (add, del), comments, checks)| {
                    let mut row = pr("hooli-inc", repo, number);
                    row.author = author.map(actor);
                    row.created_at = day(created);
                    row.updated_at = day(updated);
                    row.additions = add;
                    row.deletions = del;
                    row.comment_count = comments;
                    row.checks = checks;
                    row
                },
            )
            .collect()
    }

    #[test]
    fn every_key_sorts_both_ways_and_rows_without_a_value_go_last() {
        use SortDir::{Ascending as Up, Descending as Down};
        let prs = sortable();
        let cases = [
            (SortKey::Updated, Up, [3, 5, 4, 2, 1]),
            (SortKey::Updated, Down, [1, 2, 4, 5, 3]),
            (SortKey::Created, Up, [2, 4, 5, 1, 3]),
            (SortKey::Created, Down, [3, 1, 5, 4, 2]),
            (SortKey::Changes, Up, [3, 5, 1, 4, 2]),
            (SortKey::Changes, Down, [2, 4, 1, 5, 3]),
            // Ties (1 and 5 have four comments) keep GitHub's order.
            (SortKey::Comments, Up, [2, 4, 1, 5, 3]),
            (SortKey::Comments, Down, [3, 1, 5, 4, 2]),
            (SortKey::Checks, Up, [2, 5, 4, 1, 3]),
            (SortKey::Checks, Down, [1, 4, 2, 5, 3]),
            // Logins compare without case; a deleted author goes last.
            (SortKey::Author, Up, [2, 5, 4, 1, 3]),
            (SortKey::Author, Down, [1, 4, 5, 2, 3]),
            (SortKey::Repository, Up, [2, 4, 3, 1, 5]),
            (SortKey::Repository, Down, [5, 1, 3, 2, 4]),
        ];
        for (key, dir, want) in cases {
            assert_eq!(sorted(&prs, key, dir), want, "{key:?} {dir:?}");
        }
        let mut rows: Vec<&PrSummary> = prs.iter().collect();
        sort_rows(&mut rows, None);
        let numbers: Vec<u64> = rows.iter().map(|pr| pr.pr.number).collect();
        assert_eq!(numbers, [1, 2, 3, 4, 5], "no sort is GitHub's order");
    }

    #[test]
    fn a_column_click_goes_ascending_then_descending_then_back_to_default() {
        let up = |key| {
            Some(Sort {
                key,
                dir: SortDir::Ascending,
            })
        };
        let down = |key| {
            Some(Sort {
                key,
                dir: SortDir::Descending,
            })
        };
        assert_eq!(cycle_sort(None, SortKey::Changes), up(SortKey::Changes));
        assert_eq!(
            cycle_sort(up(SortKey::Changes), SortKey::Changes),
            down(SortKey::Changes)
        );
        assert_eq!(cycle_sort(down(SortKey::Changes), SortKey::Changes), None);
        assert_eq!(
            cycle_sort(down(SortKey::Changes), SortKey::Author),
            up(SortKey::Author)
        );
        assert_eq!(
            up(SortKey::Checks).map(Sort::describe),
            Some("Failing first")
        );
    }

    #[test]
    fn views_survive_a_save_and_a_load() {
        let dir = std::env::temp_dir().join(format!("loupe-reviews-{}", std::process::id()));
        let path = dir.join("reviews-views.json");
        let _ = std::fs::remove_file(&path);
        assert!(matches!(load(&path), Loaded::Missing));

        let now = Utc::now();
        let mut file = ViewsFile::default();
        let filters = vec![clause(Field::Org, Op::In, &["hooli-inc"])];
        let id = file.create("  Hooli  ", filters.clone(), now);
        assert!(file.rename(&id, "Hooli approved", now));
        let mut more = filters.clone();
        more.push(clause(Field::Review, Op::In, &["approved"]));
        assert!(file.update_filters(&id, more.clone(), now));
        assert!(file.set_all_open(&id, false, now));
        let largest = Some(Sort {
            key: SortKey::Changes,
            dir: SortDir::Descending,
        });
        assert!(file.set_sort(&id, largest, now));
        assert!(file.set_tab(&id, Some(ReviewTab::Mine), now));
        assert!(file.set_collapsed(&id, vec![NextStep::Draft], now));
        save(&path, &file).unwrap();

        let Loaded::Read(read) = load(&path) else {
            panic!("the saved file reads back")
        };
        assert_eq!(read.views.len(), 1);
        assert_eq!(read.views[0].name, "Hooli approved");
        assert_eq!(read.views[0].filters, more);
        assert_eq!(read.views[0].all_open, Some(false));
        assert_eq!(read.views[0].sort, largest);
        assert_eq!(read.views[0].tab, Some(ReviewTab::Mine));
        assert_eq!(read.views[0].collapsed, [NextStep::Draft]);

        std::fs::write(&path, b"{ not json").unwrap();
        assert!(matches!(load(&path), Loaded::Recovered { .. }));
        assert!(path.with_extension("json.bak").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_view_saved_before_tabs_reads_with_none() {
        let json = r#"{"version":1,"views":[{"id":"rv_1","name":"Old","filters":[],
            "createdAt":"2026-11-01T00:00:00Z","updatedAt":"2026-11-01T00:00:00Z"}]}"#;
        let file: ViewsFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.views[0].tab, None);
        assert!(file.views[0].collapsed.is_empty());
    }

    /// A pull request with these facts and nothing else.
    fn in_state(
        draft: bool,
        review: Option<ReviewDecision>,
        checks: Option<ChecksState>,
        mergeable: Mergeable,
    ) -> PrSummary {
        PrSummary {
            is_draft: draft,
            review_decision: review,
            checks,
            mergeable,
            ..pr("a", "b", 1)
        }
    }

    #[test]
    fn each_pull_request_lands_in_the_group_of_its_next_step() {
        use ChecksState::{Failing, Passing, Pending};
        use Mergeable::{Conflicting, Unknown};
        use NextStep::*;
        use ReviewDecision::{Approved, ChangesRequested as Asked, ReviewRequired};
        let ready = Mergeable::Mergeable;
        let cases = [
            // Approved, green, no conflict.
            ((false, Some(Approved), Some(Passing), ready), ReadyToMerge),
            ((false, Some(Approved), None, ready), ReadyToMerge),
            (
                (false, Some(Approved), Some(Passing), Unknown),
                ReadyToMerge,
            ),
            // Anything short of that waits on something else.
            (
                (false, Some(Approved), Some(Pending), ready),
                WaitingForReview,
            ),
            (
                (false, Some(Approved), Some(Passing), Conflicting),
                WaitingForReview,
            ),
            ((true, Some(Approved), Some(Passing), ready), Draft),
            // Failing checks come before every review.
            ((false, Some(Approved), Some(Failing), ready), ChecksFailing),
            ((false, Some(Asked), Some(Failing), ready), ChecksFailing),
            ((true, None, Some(Failing), Unknown), ChecksFailing),
            // Then changes asked for.
            ((false, Some(Asked), Some(Passing), ready), ChangesRequested),
            ((false, Some(Asked), None, Conflicting), ChangesRequested),
            (
                (true, Some(Asked), Some(Pending), Unknown),
                ChangesRequested,
            ),
            // Then a review still to come.
            (
                (false, Some(ReviewRequired), Some(Passing), ready),
                WaitingForReview,
            ),
            ((false, None, None, Unknown), WaitingForReview),
            ((false, None, Some(Pending), Conflicting), WaitingForReview),
            // A draft with nothing failing and nothing asked.
            ((true, None, None, Unknown), Draft),
            ((true, Some(ReviewRequired), Some(Passing), ready), Draft),
            ((true, None, Some(Pending), Conflicting), Draft),
        ];
        for ((draft, review, checks, mergeable), group) in cases {
            let pr = in_state(draft, review, checks, mergeable);
            assert_eq!(
                next_step(&pr),
                group,
                "draft {draft}, review {review:?}, checks {checks:?}, {mergeable:?}"
            );
        }

        // Every combination: the group's own rule holds, and no earlier
        // group's rule does.
        let reviews = [None, Some(Approved), Some(Asked), Some(ReviewRequired)];
        let checks = [None, Some(Passing), Some(Pending), Some(Failing)];
        for draft in [false, true] {
            for review in reviews {
                for check in checks {
                    for mergeable in [ready, Conflicting, Unknown] {
                        let pr = in_state(draft, review, check, mergeable);
                        let rules = [
                            !draft
                                && review == Some(Approved)
                                && matches!(check, None | Some(Passing))
                                && mergeable != Conflicting,
                            check == Some(Failing),
                            review == Some(Asked),
                            !draft,
                            true,
                        ];
                        let first = rules.iter().position(|holds| *holds).unwrap();
                        assert_eq!(next_step(&pr), NextStep::ALL[first], "{pr:?}");
                    }
                }
            }
        }
    }
}
