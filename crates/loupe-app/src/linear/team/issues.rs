//! Pure list logic for the team view, after t3code's `kanbanView.ts`:
//! paging, the exact filter on top of the server's superset, ordering, and
//! grouping into board columns or list sections.

use chrono::{DateTime, Utc};
use loupe_linear::filter::{MatchContext, matches_query};
use loupe_linear::views::{Grouping, Ordering, ViewConfig};
use loupe_linear::{IssueSummary, IssueUpdate, Priority, StateRef, Team, User};

/// Appends the issues of a new page, replacing in place any issue already
/// listed (Linear can repeat one across pages when it changes mid-scroll).
/// An older copy never replaces a newer one, so a slow response cannot undo
/// a change Linear already confirmed.
pub(super) fn merge_page(existing: &mut Vec<IssueSummary>, incoming: Vec<IssueSummary>) {
    for issue in incoming {
        match existing.iter_mut().find(|listed| listed.id == issue.id) {
            Some(listed) => {
                if issue.updated_at >= listed.updated_at {
                    *listed = issue;
                }
            }
            None => existing.push(issue),
        }
    }
}

/// Keeps the newer copy of each issue in `fresh` from `old`: a first page
/// requested before a drop was confirmed must not bring the card back.
pub(super) fn keep_newer(fresh: &mut [IssueSummary], old: &[IssueSummary]) {
    for issue in fresh {
        if let Some(newer) = old
            .iter()
            .find(|listed| listed.id == issue.id && listed.updated_at > issue.updated_at)
        {
            *issue = newer.clone();
        }
    }
}

/// What evaluates "me" and "current cycle" for `team` at `now`.
pub(super) fn match_context(team: &Team, me: Option<String>, now: DateTime<Utc>) -> MatchContext {
    MatchContext {
        me,
        current_cycles: team
            .cycles
            .iter()
            .filter(|cycle| cycle.starts_at <= now && now < cycle.ends_at)
            .map(|cycle| cycle.id.clone())
            .collect(),
    }
}

/// The issues `config` keeps, in its order.
pub(super) fn visible<'a>(
    issues: &'a [IssueSummary],
    config: &ViewConfig,
    context: &MatchContext,
    now: DateTime<Utc>,
) -> Vec<&'a IssueSummary> {
    let query = config.query(now);
    let mut kept: Vec<&IssueSummary> = issues
        .iter()
        .filter(|issue| matches_query(issue, &query, context, now))
        .collect();
    sort(&mut kept, config.order_by, config.sort_direction);
    kept
}

/// Linear's 0 is "no priority"; it sorts after urgent (1) through low (4).
fn priority_rank(priority: Priority) -> i64 {
    match priority {
        Priority::None => 5,
        other => other.to_wire(),
    }
}

/// Orders issues as the view says. Ties keep the newest update first.
pub(super) fn sort(
    issues: &mut [&IssueSummary],
    order: Ordering,
    direction: Option<loupe_linear::views::SortDirection>,
) {
    use loupe_linear::views::SortDirection;
    let natural = match order {
        Ordering::Priority | Ordering::DueDate | Ordering::Manual => SortDirection::Ascending,
        _ => SortDirection::Descending,
    };
    let reverse = direction.is_some_and(|direction| direction != natural);
    issues.sort_by(|a, b| {
        let missing = match order {
            Ordering::Priority => (a.priority == Priority::None, b.priority == Priority::None),
            Ordering::DueDate => (a.due_date.is_none(), b.due_date.is_none()),
            _ => (false, false),
        };
        if missing.0 != missing.1 {
            return missing.0.cmp(&missing.1);
        }
        let primary = match order {
            Ordering::Priority => priority_rank(a.priority).cmp(&priority_rank(b.priority)),
            Ordering::Updated => b.updated_at.cmp(&a.updated_at),
            Ordering::Manual => a.sort_order.total_cmp(&b.sort_order),
            Ordering::Created => b.created_at.cmp(&a.created_at),
            Ordering::DueDate => match (a.due_date, b.due_date) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            },
        };
        let primary = if reverse { primary.reverse() } else { primary };
        primary.then_with(|| b.updated_at.cmp(&a.updated_at))
    });
}

/// How a group's header marks it.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum GroupMark {
    State(StateRef),
    Priority(Priority),
    Person(Option<User>),
    /// A label's or project's color, from Linear's data.
    Swatch(Option<String>),
    Plain,
}

/// One board column or list section.
pub(super) struct Group<'a> {
    pub key: String,
    pub title: String,
    pub mark: GroupMark,
    pub issues: Vec<&'a IssueSummary>,
}

fn group<'a>(key: impl Into<String>, title: impl Into<String>, mark: GroupMark) -> Group<'a> {
    Group {
        key: key.into(),
        title: title.into(),
        mark,
        issues: Vec::new(),
    }
}

/// Adds `issue` to the group with `candidate`'s key, or starts that group.
fn place<'a>(groups: &mut Vec<Group<'a>>, mut candidate: Group<'a>, issue: &'a IssueSummary) {
    match groups.iter_mut().find(|known| known.key == candidate.key) {
        Some(known) => known.issues.push(issue),
        None => {
            candidate.issues.push(issue);
            groups.push(candidate);
        }
    }
}

/// Splits `issues` (already in view order) into groups. Status groups follow
/// the team's states and keep empty ones; other groupings list the values
/// the issues have, then "No …" last. An issue with several labels sits in
/// each label's group.
pub(super) fn group_issues<'a>(
    issues: &[&'a IssueSummary],
    grouping: Grouping,
    team: &Team,
) -> Vec<Group<'a>> {
    let mut groups: Vec<Group<'a>> = Vec::new();
    match grouping {
        Grouping::None => {
            let mut all = group("all", "All issues", GroupMark::Plain);
            all.issues = issues.to_vec();
            return vec![all];
        }
        Grouping::Status => {
            groups = team
                .states
                .iter()
                .map(|state| {
                    group(
                        state.id.clone(),
                        state.name.clone(),
                        GroupMark::State(StateRef {
                            id: state.id.clone(),
                            name: state.name.clone(),
                            kind: state.kind,
                            color: state.color.clone(),
                        }),
                    )
                })
                .collect();
            for issue in issues {
                let candidate = group(
                    issue.state.id.clone(),
                    issue.state.name.clone(),
                    GroupMark::State(issue.state.clone()),
                );
                place(&mut groups, candidate, issue);
            }
            return groups;
        }
        Grouping::Priority => {
            groups = Priority::ALL
                .iter()
                .map(|priority| {
                    group(
                        priority.to_wire().to_string(),
                        priority.label(),
                        GroupMark::Priority(*priority),
                    )
                })
                .collect();
            for issue in issues {
                let candidate = group(
                    issue.priority.to_wire().to_string(),
                    issue.priority.label(),
                    GroupMark::Priority(issue.priority),
                );
                place(&mut groups, candidate, issue);
            }
            return groups;
        }
        Grouping::Assignee => {
            for issue in issues {
                let candidate = match &issue.assignee {
                    Some(user) => group(
                        user.id.clone(),
                        user.name.clone(),
                        GroupMark::Person(Some(user.clone())),
                    ),
                    None => group("", "No assignee", GroupMark::Person(None)),
                };
                place(&mut groups, candidate, issue);
            }
        }
        Grouping::Label => {
            for issue in issues {
                if issue.labels.is_empty() {
                    place(
                        &mut groups,
                        group("", "No label", GroupMark::Swatch(None)),
                        issue,
                    );
                }
                for label in &issue.labels {
                    let candidate = group(
                        label.id.clone(),
                        label.name.clone(),
                        GroupMark::Swatch(Some(label.color.clone())),
                    );
                    place(&mut groups, candidate, issue);
                }
            }
        }
        Grouping::Project => {
            for issue in issues {
                let candidate = match &issue.project {
                    Some(project) => group(
                        project.id.clone(),
                        project.name.clone(),
                        GroupMark::Swatch(project.color.clone()),
                    ),
                    None => group("", "No project", GroupMark::Swatch(None)),
                };
                place(&mut groups, candidate, issue);
            }
        }
        Grouping::Cycle => {
            for issue in issues {
                let candidate = match &issue.cycle {
                    Some(cycle) => group(
                        cycle.id.clone(),
                        cycle
                            .name
                            .clone()
                            .unwrap_or_else(|| format!("Cycle {}", cycle.number)),
                        GroupMark::Plain,
                    ),
                    None => group("", "No cycle", GroupMark::Plain),
                };
                place(&mut groups, candidate, issue);
            }
        }
    }
    // The "No …" group (empty key) goes last; the rest keep first appearance.
    groups.sort_by_key(|group| group.key.is_empty());
    groups
}

/// Puts `groups` in the board's column order: the groups `order` names, in
/// its order, then the others in the grouping's own order.
pub(super) fn order_columns(groups: &mut [Group<'_>], order: &[String]) {
    if order.is_empty() {
        return;
    }
    groups.sort_by_key(|group| {
        order
            .iter()
            .position(|key| *key == group.key)
            .unwrap_or(usize::MAX)
    });
}

/// The column order to save once column `moved` is dropped on column
/// `target`: it takes the target's place, and the columns in between shift
/// toward where it came from. `natural` is every column of the grouping in
/// its own order, and `order` the order saved so far. The places the order
/// gives columns not on the board now (another grouping's, or ones a filter
/// leaves out) are kept after these. An order that comes back to the
/// grouping's own, with nothing else in it, is saved as none, so the view is
/// as it was. `None` when nothing moves.
pub(super) fn reorder_columns(
    natural: &[String],
    order: &[String],
    moved: &str,
    target: &str,
) -> Option<Vec<String>> {
    if moved == target {
        return None;
    }
    let mut keys = natural.to_vec();
    keys.sort_by_key(|key| order.iter().position(|o| o == key).unwrap_or(usize::MAX));
    let from = keys.iter().position(|key| key == moved)?;
    let to = keys.iter().position(|key| key == target)?;
    let key = keys.remove(from);
    keys.insert(to, key);
    let elsewhere: Vec<String> = order
        .iter()
        .filter(|key| !natural.contains(key))
        .cloned()
        .collect();
    if keys == natural && elsewhere.is_empty() {
        return Some(Vec::new());
    }
    keys.extend(elsewhere);
    Some(keys)
}

/// Where a dragged card was let go: a column, and the card it lands above
/// (`None` for the end of the column).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct DropTarget {
    pub group_key: String,
    pub before: Option<String>,
}

/// What a drop sends to Linear.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct DropPlan {
    pub update: IssueUpdate,
    /// A reorder inside a column under another ordering: the view switches to
    /// manual order, as Linear does, so the card stays where it was put.
    pub switch_to_manual: bool,
}

/// The edit that dropping issue `dragged`, taken from group `from_key`, on
/// `target` makes, after Linear's board: a new column sets the field the
/// board groups by, and a drop where position counts (the same column, or
/// any column in manual order) sets the manual order between the cards
/// around it. `None` when the drop changes nothing.
pub(super) fn drop_plan(
    groups: &[Group<'_>],
    grouping: Grouping,
    order: Ordering,
    dragged: &str,
    from_key: &str,
    target: &DropTarget,
) -> Option<DropPlan> {
    let from = groups.iter().find(|group| group.key == from_key)?;
    let issue = from.issues.iter().find(|issue| issue.id == dragged)?;
    let to = groups.iter().find(|group| group.key == target.group_key)?;
    let same = from.key == to.key;
    // Let go where it already shows: nothing to do.
    if same
        && match target.before.as_deref() {
            Some(before) if before == dragged => true,
            Some(before) => to
                .issues
                .windows(2)
                .any(|pair| pair[0].id == dragged && pair[1].id == before),
            None => to.issues.last().is_some_and(|last| last.id == dragged),
        }
    {
        return None;
    }
    let switch_to_manual = same && order != Ordering::Manual;
    let mut update = if same {
        IssueUpdate::default()
    } else {
        field_update(issue, grouping, from, to)?
    };
    if same || order == Ordering::Manual {
        // The neighbors in manual order, which is what the drop writes: under
        // another ordering the card then sits right above the card it was
        // dropped on once the view switches to manual.
        let mut ordered = to.issues.clone();
        ordered.sort_by(|a, b| a.sort_order.total_cmp(&b.sort_order));
        let column: Vec<&IssueSummary> = ordered
            .iter()
            .copied()
            .filter(|other| other.id != dragged)
            .collect();
        let index = match target.before.as_deref() {
            // On itself: where it already is.
            Some(before) if before == dragged => {
                ordered.iter().position(|other| other.id == dragged)?
            }
            Some(before) => column
                .iter()
                .position(|other| other.id == before)
                .unwrap_or(column.len()),
            None => column.len(),
        };
        let unmoved = same && ordered.get(index).is_some_and(|at| at.id == dragged);
        if !unmoved {
            let above = index.checked_sub(1).map(|at| column[at].sort_order);
            let below = column.get(index).map(|other| other.sort_order);
            update.sort_order = sort_order_between(above, below);
        }
    }
    // A reorder whose manual position is already right still switches the
    // view to manual, where the card shows there; it sends nothing.
    if update == IssueUpdate::default() && !switch_to_manual {
        return None;
    }
    Some(DropPlan {
        update,
        switch_to_manual,
    })
}

/// The grouped field set to `to`'s value. A label column swaps the label the
/// card was dragged from for the new one. `None` for a grouping a drop
/// cannot change.
fn field_update(
    issue: &IssueSummary,
    grouping: Grouping,
    from: &Group<'_>,
    to: &Group<'_>,
) -> Option<IssueUpdate> {
    let id = (!to.key.is_empty()).then(|| to.key.clone());
    let mut update = IssueUpdate::default();
    match (grouping, &to.mark) {
        (Grouping::Status, _) => update.state_id = Some(id?),
        (Grouping::Priority, GroupMark::Priority(priority)) => update.priority = Some(*priority),
        (Grouping::Assignee, _) => update.assignee_id = Some(id),
        (Grouping::Project, _) => update.project_id = Some(id),
        (Grouping::Cycle, _) => update.cycle_id = Some(id),
        (Grouping::Label, _) => {
            let mut ids: Vec<String> = issue
                .labels
                .iter()
                .map(|label| label.id.clone())
                .filter(|label| *label != from.key)
                .collect();
            if let Some(id) = id
                && !ids.contains(&id)
            {
                ids.push(id);
            }
            update.label_ids = Some(ids);
        }
        _ => return None,
    }
    Some(update)
}

/// A manual order between two neighbors (lower sorts first). `None` in an
/// empty column, where any order fits.
fn sort_order_between(above: Option<f64>, below: Option<f64>) -> Option<f64> {
    match (above, below) {
        (Some(above), Some(below)) => Some((above + below) / 2.),
        (Some(above), None) => Some(above + 1.),
        (None, Some(below)) => Some(below - 1.),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, NaiveDate, Utc};
    use loupe_linear::filter::{FilterClause, ME, MatchContext, SetField, SetOp};
    use loupe_linear::views::{CompletedWindow, Grouping, Ordering, ViewConfig};
    use loupe_linear::{
        IssueSummary, Label, Priority, StateRef, StateType, Team, TeamRef, User, WorkflowState,
    };

    use loupe_linear::IssueUpdate;

    use super::{
        DropTarget, drop_plan, group_issues, keep_newer, merge_page, order_columns,
        reorder_columns, sort, visible,
    };

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("valid timestamp")
            .to_utc()
    }

    fn state(id: &str, kind: StateType) -> WorkflowState {
        WorkflowState {
            id: id.into(),
            name: id.to_uppercase(),
            kind,
            position: 0.,
            color: "#888888".into(),
        }
    }

    fn team() -> Team {
        Team {
            id: "t1".into(),
            key: "ENG".into(),
            name: "Engineering".into(),
            states: vec![
                state("todo", StateType::Unstarted),
                state("doing", StateType::Started),
                state("done", StateType::Completed),
            ],
            members: Vec::new(),
            labels: Vec::new(),
            projects: Vec::new(),
            cycles: Vec::new(),
            label_groups: Vec::new(),
            estimation: Default::default(),
        }
    }

    fn issue(id: &str, state_id: &str, priority: Priority, updated: &str) -> IssueSummary {
        let kind = match state_id {
            "doing" => StateType::Started,
            "done" => StateType::Completed,
            _ => StateType::Unstarted,
        };
        IssueSummary {
            id: id.into(),
            identifier: format!("ENG-{id}"),
            title: format!("Issue {id}"),
            priority,
            sort_order: 0.,
            state: StateRef {
                id: state_id.into(),
                name: state_id.to_uppercase(),
                kind,
                color: "#888888".into(),
            },
            team: TeamRef {
                id: "t1".into(),
                key: "ENG".into(),
                name: "Engineering".into(),
            },
            assignee: None,
            labels: Vec::new(),
            project: None,
            cycle: None,
            estimate: None,
            due_date: None,
            url: String::new(),
            branch_name: String::new(),
            created_at: at("2026-10-01T00:00:00Z"),
            updated_at: at(updated),
            creator_id: None,
            parent_id: None,
            has_sub_issues: false,
            milestone: None,
            closed_at: None,
            pull_requests: Vec::new(),
            pull_requests_complete: true,
        }
    }

    fn ids(issues: &[&IssueSummary]) -> Vec<String> {
        issues.iter().map(|issue| issue.id.clone()).collect()
    }

    #[test]
    fn next_page_is_appended_without_duplicates() {
        let mut listed = vec![issue("1", "todo", Priority::Low, "2026-11-01T00:00:00Z")];
        let mut moved = issue("1", "doing", Priority::Low, "2026-11-02T00:00:00Z");
        moved.title = "Moved".into();
        merge_page(
            &mut listed,
            vec![
                moved,
                issue("2", "todo", Priority::Low, "2026-11-01T00:00:00Z"),
            ],
        );
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].title, "Moved");
    }

    #[test]
    fn priority_order_puts_none_last_and_breaks_ties_by_update() {
        let a = issue("a", "todo", Priority::None, "2026-11-09T00:00:00Z");
        let b = issue("b", "todo", Priority::Low, "2026-11-01T00:00:00Z");
        let c = issue("c", "todo", Priority::Urgent, "2026-11-02T00:00:00Z");
        let d = issue("d", "todo", Priority::Low, "2026-11-05T00:00:00Z");
        let mut list = vec![&a, &b, &c, &d];
        sort(&mut list, Ordering::Priority, None);
        assert_eq!(ids(&list), ["c", "d", "b", "a"]);
        let mut due_a = a.clone();
        due_a.due_date = NaiveDate::from_ymd_opt(2026, 11, 20);
        let mut list = vec![&b, &due_a];
        sort(&mut list, Ordering::DueDate, None);
        assert_eq!(ids(&list), ["a", "b"]);
    }

    #[test]
    fn explicit_direction_orders_the_actual_issue_list() {
        use loupe_linear::views::SortDirection;
        let old = issue("old", "todo", Priority::Low, "2026-11-01T00:00:00Z");
        let new = issue("new", "todo", Priority::Low, "2026-11-09T00:00:00Z");
        let mut list = vec![&new, &old];
        sort(&mut list, Ordering::Updated, Some(SortDirection::Ascending));
        assert_eq!(ids(&list), ["old", "new"]);
        sort(
            &mut list,
            Ordering::Updated,
            Some(SortDirection::Descending),
        );
        assert_eq!(ids(&list), ["new", "old"]);
    }

    #[test]
    fn status_groups_follow_the_team_and_keep_empty_states() {
        let team = team();
        let a = issue("a", "doing", Priority::Low, "2026-11-01T00:00:00Z");
        let b = issue("b", "gone", Priority::Low, "2026-11-01T00:00:00Z");
        let groups = group_issues(&[&a, &b], Grouping::Status, &team);
        let keys: Vec<&str> = groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, ["todo", "doing", "done", "gone"]);
        assert!(groups[0].issues.is_empty());
        assert_eq!(ids(&groups[1].issues), ["a"]);
    }

    #[test]
    fn label_groups_hold_an_issue_once_per_label_and_none_goes_last() {
        let team = team();
        let label = |id: &str| Label {
            id: id.into(),
            name: id.into(),
            color: "#111111".into(),
            parent_id: None,
        };
        let bare = issue("a", "todo", Priority::Low, "2026-11-01T00:00:00Z");
        let mut both = issue("b", "todo", Priority::Low, "2026-11-01T00:00:00Z");
        both.labels = vec![label("bug"), label("ui")];
        let groups = group_issues(&[&bare, &both], Grouping::Label, &team);
        let keys: Vec<&str> = groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, ["bug", "ui", ""]);
        assert_eq!(groups[2].title, "No label");
    }

    #[test]
    fn visible_applies_clauses_and_the_closed_window() {
        let mut mine = issue("a", "todo", Priority::Low, "2026-11-01T00:00:00Z");
        mine.assignee = Some(User {
            id: "u-me".into(),
            name: "Ada".into(),
            display_name: "ada".into(),
            avatar_url: None,
        });
        let theirs = issue("b", "todo", Priority::Low, "2026-11-01T00:00:00Z");
        let mut old_done = issue("c", "done", Priority::Low, "2026-01-01T00:00:00Z");
        old_done.closed_at = Some(at("2026-01-01T00:00:00Z"));
        let issues = vec![mine, theirs, old_done];
        let context = MatchContext {
            me: Some("u-me".into()),
            ..MatchContext::default()
        };
        let now = at("2026-11-10T00:00:00Z");
        let all = ViewConfig {
            completed: CompletedWindow::PastMonth,
            ..ViewConfig::default()
        };
        assert_eq!(ids(&visible(&issues, &all, &context, now)), ["a", "b"]);
        let mine_only = ViewConfig {
            filters: vec![FilterClause::Set {
                field: SetField::Assignee,
                op: SetOp::In,
                values: vec![ME.into()],
            }],
            ..all
        };
        assert_eq!(ids(&visible(&issues, &mine_only, &context, now)), ["a"]);
    }

    fn ordered(id: &str, state_id: &str, sort_order: f64) -> IssueSummary {
        let mut issue = issue(id, state_id, Priority::None, "2026-11-01T00:00:00Z");
        issue.sort_order = sort_order;
        issue
    }

    fn target(group_key: &str, before: Option<&str>) -> DropTarget {
        DropTarget {
            group_key: group_key.into(),
            before: before.map(Into::into),
        }
    }

    #[test]
    fn a_new_status_column_sets_only_the_state_outside_manual_order() {
        let team = team();
        let (a, b) = (ordered("a", "todo", 1.), ordered("b", "doing", 2.));
        let groups = group_issues(&[&a, &b], Grouping::Status, &team);
        let plan = drop_plan(
            &groups,
            Grouping::Status,
            Ordering::Priority,
            "a",
            "todo",
            &target("doing", Some("b")),
        )
        .expect("a move");
        assert_eq!(
            plan.update,
            IssueUpdate {
                state_id: Some("doing".into()),
                ..IssueUpdate::default()
            }
        );
        assert!(!plan.switch_to_manual);
    }

    #[test]
    fn manual_order_places_the_card_between_its_new_neighbors() {
        let team = team();
        let a = ordered("a", "todo", 5.);
        let (b, c) = (ordered("b", "doing", 1.), ordered("c", "doing", 2.));
        let groups = group_issues(&[&b, &c, &a], Grouping::Status, &team);
        let plan = |target: DropTarget| {
            drop_plan(
                &groups,
                Grouping::Status,
                Ordering::Manual,
                "a",
                "todo",
                &target,
            )
            .expect("a move")
            .update
        };
        let between = plan(target("doing", Some("c")));
        assert_eq!(between.state_id.as_deref(), Some("doing"));
        assert_eq!(between.sort_order, Some(1.5));
        assert_eq!(plan(target("doing", Some("b"))).sort_order, Some(0.));
        assert_eq!(plan(target("doing", None)).sort_order, Some(3.));
        let empty = plan(target("done", None));
        assert_eq!(empty.state_id.as_deref(), Some("done"));
        assert_eq!(empty.sort_order, None);
    }

    #[test]
    fn reordering_a_column_switches_to_manual_and_a_drop_in_place_does_nothing() {
        let team = team();
        let (a, b, c) = (
            ordered("a", "todo", 1.),
            ordered("b", "todo", 2.),
            ordered("c", "todo", 3.),
        );
        let groups = group_issues(&[&a, &b, &c], Grouping::Status, &team);
        let plan = |before: Option<&str>| {
            drop_plan(
                &groups,
                Grouping::Status,
                Ordering::Priority,
                "a",
                "todo",
                &target("todo", before),
            )
        };
        let moved = plan(Some("c")).expect("a move");
        assert_eq!(
            moved.update,
            IssueUpdate {
                sort_order: Some(2.5),
                ..IssueUpdate::default()
            }
        );
        assert!(moved.switch_to_manual);
        assert_eq!(plan(None).expect("a move").update.sort_order, Some(4.));
        assert_eq!(plan(Some("a")), None);
        assert_eq!(plan(Some("b")), None);
    }

    #[test]
    fn other_groupings_set_their_field() {
        let team = team();
        let mut mine = ordered("a", "todo", 1.);
        mine.assignee = Some(User {
            id: "u1".into(),
            name: "Ada".into(),
            display_name: "ada".into(),
            avatar_url: None,
        });
        mine.labels = vec![Label {
            id: "bug".into(),
            name: "bug".into(),
            color: "#111111".into(),
            parent_id: None,
        }];
        let mut urgent = ordered("b", "todo", 2.);
        urgent.priority = Priority::Urgent;
        let mut ui = ordered("c", "todo", 3.);
        ui.labels = vec![Label {
            id: "ui".into(),
            name: "ui".into(),
            color: "#222222".into(),
            parent_id: None,
        }];
        let unassigned = ordered("d", "todo", 4.);
        let issues = [&mine, &urgent, &ui, &unassigned];
        let plan = |grouping, from: &str, to: &str| {
            let groups = group_issues(&issues, grouping, &team);
            drop_plan(
                &groups,
                grouping,
                Ordering::Priority,
                "a",
                from,
                &target(to, None),
            )
            .expect("a move")
            .update
        };
        assert_eq!(
            plan(Grouping::Priority, "0", "1").priority,
            Some(Priority::Urgent)
        );
        assert_eq!(plan(Grouping::Assignee, "u1", "").assignee_id, Some(None));
        assert_eq!(
            plan(Grouping::Label, "bug", "ui").label_ids,
            Some(vec!["ui".to_owned()])
        );
    }

    #[test]
    fn a_reorder_under_another_ordering_lands_above_the_card_in_manual_order() {
        let team = team();
        // Shown by priority as a, b, c, d; manual order is b, c, d, a.
        let (mut a, mut b, mut c, mut d) = (
            ordered("a", "todo", 100.),
            ordered("b", "todo", 10.),
            ordered("c", "todo", 20.),
            ordered("d", "todo", 30.),
        );
        a.priority = Priority::Urgent;
        b.priority = Priority::High;
        c.priority = Priority::Medium;
        d.priority = Priority::Low;
        let groups = group_issues(&[&a, &b, &c, &d], Grouping::Status, &team);
        let plan = drop_plan(
            &groups,
            Grouping::Status,
            Ordering::Priority,
            "d",
            "todo",
            &target("todo", Some("b")),
        )
        .expect("a move");
        assert!(plan.switch_to_manual);
        let moved = plan.update.sort_order.expect("a manual order");
        assert!(moved < b.sort_order, "{moved} must sort right above b");
        // d already sits right above a in manual order: only the view
        // switches.
        let in_place = drop_plan(
            &groups,
            Grouping::Status,
            Ordering::Priority,
            "d",
            "todo",
            &target("todo", Some("a")),
        )
        .expect("a switch to manual");
        assert!(in_place.switch_to_manual);
        assert_eq!(in_place.update, IssueUpdate::default());
    }

    #[test]
    fn an_older_copy_never_replaces_a_newer_one() {
        let confirmed = issue("1", "doing", Priority::Low, "2026-11-02T00:00:00Z");
        let stale = issue("1", "todo", Priority::Low, "2026-11-01T00:00:00Z");
        let mut listed = vec![confirmed.clone()];
        merge_page(&mut listed, vec![stale.clone()]);
        assert_eq!(listed[0].state.id, "doing");
        let mut fresh = vec![stale];
        keep_newer(&mut fresh, &[confirmed]);
        assert_eq!(fresh[0].state.id, "doing");
    }

    fn keys(list: &[&str]) -> Vec<String> {
        list.iter().map(|key| (*key).to_owned()).collect()
    }

    #[test]
    fn a_dropped_column_takes_the_place_of_the_one_it_lands_on() {
        let natural = keys(&["todo", "doing", "review", "done"]);
        // To the right: lands after the target, the ones between shift left.
        assert_eq!(
            reorder_columns(&natural, &[], "todo", "review"),
            Some(keys(&["doing", "review", "todo", "done"]))
        );
        // To the left: lands before it.
        assert_eq!(
            reorder_columns(&natural, &[], "done", "doing"),
            Some(keys(&["todo", "done", "doing", "review"]))
        );
        // Over a saved order.
        let saved = keys(&["done", "todo", "doing", "review"]);
        assert_eq!(
            reorder_columns(&natural, &saved, "review", "todo"),
            Some(keys(&["done", "review", "todo", "doing"]))
        );
        // Dropped on itself, or on a column that is not there: nothing moves.
        assert_eq!(reorder_columns(&natural, &saved, "todo", "todo"), None);
        assert_eq!(reorder_columns(&natural, &saved, "todo", "gone"), None);
        // Back to the grouping's own order: no order is saved.
        assert_eq!(
            reorder_columns(&natural, &keys(&["doing", "todo"]), "doing", "todo"),
            Some(Vec::new())
        );
        // The places of columns not on the board now (a filter leaves "x"
        // out, "p1" belongs to another grouping) are kept, after these.
        let saved = keys(&["p1", "doing", "x", "todo", "review", "done"]);
        assert_eq!(
            reorder_columns(&natural, &saved, "doing", "todo"),
            Some(keys(&["todo", "doing", "review", "done", "p1", "x"]))
        );
    }

    #[test]
    fn the_board_shows_its_columns_in_the_saved_order() {
        let team = team();
        let issues = [issue("a", "todo", Priority::None, "2026-01-01T00:00:00Z")];
        let refs: Vec<&IssueSummary> = issues.iter().collect();
        let titles = |order: &[String]| {
            let mut groups = group_issues(&refs, Grouping::Status, &team);
            order_columns(&mut groups, order);
            groups
                .into_iter()
                .map(|group| group.key)
                .collect::<Vec<_>>()
        };
        let natural = titles(&[]);
        assert_eq!(natural.len(), team.states.len());
        // A column the order does not name (a state added since) comes
        // after the named ones, in the grouping's order.
        let last = natural.last().cloned().expect("a column");
        let order = vec![last.clone(), natural[0].clone()];
        let shown = titles(&order);
        assert_eq!(shown[0], last);
        assert_eq!(shown[1], natural[0]);
        assert_eq!(&shown[2..], &natural[1..natural.len() - 1]);
        // The order a drop saves is the order the board then shows.
        let saved = reorder_columns(&natural, &[], &natural[0], &last).expect("moved");
        assert_eq!(titles(&saved), saved);
    }
}
