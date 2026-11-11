//! The issue as the properties panel shows it while edits are in flight. Pure
//! and GPUI-free: the server's last confirmed value plus every unconfirmed
//! edit applied in order.

use loupe_linear::{
    CycleRef, IssueSummary, IssueUpdate, Label, Project, StateRef, Team, User, WorkflowState,
};

/// What the user asked for. A label click records whether the user wants that
/// one label on or off, and goes to Linear as that one label added or
/// removed, so it never undoes another edit's labels (an earlier one of this
/// page, or one made from a menu or the palette meanwhile).
#[derive(Clone, Debug, PartialEq)]
enum Change {
    Update(Box<IssueUpdate>),
    SetLabel { label_id: String, present: bool },
}

#[derive(Clone, Debug, PartialEq)]
struct PendingEdit {
    seq: u64,
    change: Change,
    /// The request as sent, once it is in flight.
    sent: Option<IssueUpdate>,
}

/// An issue with edits that have been sent but not answered yet.
///
/// The view calls [`begin`](Self::begin) or
/// [`begin_label`](Self::begin_label) when the user changes a
/// property, shows [`displayed`](Self::displayed), and later calls
/// [`confirm`](Self::confirm) or [`fail`](Self::fail) with the edit's number.
/// The view sends edits one at a time, in the order the user made them, with
/// [`take_next`](Self::take_next): two concurrent requests could reach Linear
/// in either order, and then the display would show a value the server does
/// not hold.
#[derive(Clone, Debug, PartialEq)]
pub struct OptimisticIssue {
    confirmed: IssueSummary,
    pending: Vec<PendingEdit>,
    next_seq: u64,
}

impl OptimisticIssue {
    pub fn new(confirmed: IssueSummary) -> Self {
        Self {
            confirmed,
            pending: Vec::new(),
            next_seq: 0,
        }
    }

    /// The issue as the server last confirmed it, without pending edits.
    pub fn confirmed(&self) -> &IssueSummary {
        &self.confirmed
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Records an edit that is about to be sent and returns its number.
    pub fn begin(&mut self, update: IssueUpdate) -> u64 {
        self.push(Change::Update(Box::new(update)))
    }

    /// Records that the user wants `label_id` on the issue (`present`) or off
    /// it.
    pub fn begin_label(&mut self, label_id: String, present: bool) -> u64 {
        self.push(Change::SetLabel { label_id, present })
    }

    fn push(&mut self, change: Change) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.pending.push(PendingEdit {
            seq,
            change,
            sent: None,
        });
        seq
    }

    /// The oldest edit not sent yet, as the request to send, or `None` while
    /// an edit is still waiting for its answer.
    pub fn take_next(&mut self) -> Option<(u64, IssueUpdate)> {
        if self.pending.iter().any(|edit| edit.sent.is_some()) {
            return None;
        }
        let edit = self.pending.first_mut()?;
        let update = match &edit.change {
            Change::Update(update) => (**update).clone(),
            Change::SetLabel { label_id, present } => label_delta(label_id, *present),
        };
        edit.sent = Some(update.clone());
        Some((edit.seq, update))
    }

    /// The server accepted edit `seq` and returned the issue.
    ///
    /// Only the fields that edit changed are taken from `server`. Pending
    /// edits that are still unanswered keep showing on top. An unknown or
    /// unsent `seq` is ignored.
    pub fn confirm(&mut self, seq: u64, server: IssueSummary) {
        let Some(index) = self
            .pending
            .iter()
            .position(|edit| edit.seq == seq && edit.sent.is_some())
        else {
            return;
        };
        let edit = self.pending.remove(index);
        if let Some(update) = &edit.sent {
            adopt(&mut self.confirmed, update, server);
        }
    }

    /// The server rejected edit `seq`: the display drops that change and
    /// keeps every other pending edit.
    pub fn fail(&mut self, seq: u64) {
        self.pending.retain(|edit| edit.seq != seq);
    }

    /// The confirmed issue with every pending edit applied in order. A
    /// picked id the team's catalog does not list keeps the confirmed value.
    pub fn displayed(&self, team: &Team) -> IssueSummary {
        let mut issue = self.confirmed.clone();
        for edit in &self.pending {
            match &edit.change {
                Change::Update(update) => apply(&mut issue, update, team),
                Change::SetLabel { label_id, present } => {
                    set_label(&mut issue, label_id, *present, team)
                }
            }
        }
        issue
    }
}

/// The edit that puts `label_id` on the issue (`present`) or takes it off,
/// leaving its other labels as Linear holds them.
pub(in crate::linear) fn label_delta(label_id: &str, present: bool) -> IssueUpdate {
    let ids = vec![label_id.to_owned()];
    if present {
        IssueUpdate {
            added_label_ids: ids,
            ..IssueUpdate::default()
        }
    } else {
        IssueUpdate {
            removed_label_ids: ids,
            ..IssueUpdate::default()
        }
    }
}

fn set_label(issue: &mut IssueSummary, label_id: &str, present: bool, team: &Team) {
    if !present {
        issue.labels.retain(|label| label.id != label_id);
    } else if !issue.labels.iter().any(|label| label.id == label_id)
        && let Some(label) = team.labels.iter().find(|label| label.id == label_id)
    {
        issue.labels.push(label.clone());
    }
}

/// Shows `update` on `issue` before Linear answers. A picked id the team's
/// catalog does not list keeps the issue's value.
pub(in crate::linear) fn apply(issue: &mut IssueSummary, update: &IssueUpdate, team: &Team) {
    if let Some(id) = &update.state_id
        && let Some(state) = team.states.iter().find(|state| &state.id == id)
    {
        issue.state = state_ref(state);
    }
    if let Some(priority) = update.priority {
        issue.priority = priority;
    }
    match &update.assignee_id {
        Some(None) => issue.assignee = None,
        Some(Some(id)) => {
            if let Some(member) = find_user(team, id) {
                issue.assignee = Some(member);
            }
        }
        None => {}
    }
    if let Some(ids) = &update.label_ids {
        let current = std::mem::take(&mut issue.labels);
        issue.labels = ids
            .iter()
            .filter_map(|id| {
                team.labels
                    .iter()
                    .chain(&current)
                    .find(|label| &label.id == id)
                    .cloned()
            })
            .collect::<Vec<Label>>();
    }
    for id in &update.added_label_ids {
        set_label(issue, id, true, team);
    }
    for id in &update.removed_label_ids {
        set_label(issue, id, false, team);
    }
    if let Some(estimate) = update.estimate {
        issue.estimate = estimate;
    }
    if let Some(due_date) = update.due_date {
        issue.due_date = due_date;
    }
    match &update.project_id {
        Some(None) => issue.project = None,
        Some(Some(id)) => {
            if let Some(project) = team.projects.iter().find(|project| &project.id == id) {
                issue.project = Some(Project {
                    id: project.id.clone(),
                    name: project.name.clone(),
                    color: project.color.clone(),
                });
            }
        }
        None => {}
    }
    match &update.cycle_id {
        Some(None) => issue.cycle = None,
        Some(Some(id)) => {
            if let Some(cycle) = team.cycles.iter().find(|cycle| &cycle.id == id) {
                issue.cycle = Some(CycleRef {
                    id: cycle.id.clone(),
                    number: cycle.number,
                    name: cycle.name.clone(),
                });
            }
        }
        None => {}
    }
    if let Some(title) = &update.title {
        issue.title = title.clone();
    }
    if let Some(sort_order) = update.sort_order {
        issue.sort_order = sort_order;
    }
}

/// The embedded form of a catalog state, as an issue carries it.
pub(super) fn state_ref(state: &WorkflowState) -> StateRef {
    StateRef {
        id: state.id.clone(),
        name: state.name.clone(),
        kind: state.kind,
        color: state.color.clone(),
    }
}

fn find_user(team: &Team, id: &str) -> Option<User> {
    team.members.iter().find(|member| member.id == id).cloned()
}

fn adopt(confirmed: &mut IssueSummary, update: &IssueUpdate, server: IssueSummary) {
    let IssueSummary {
        title,
        priority,
        state,
        assignee,
        labels,
        project,
        cycle,
        estimate,
        due_date,
        sort_order,
        updated_at,
        ..
    } = server;
    if update.state_id.is_some() {
        confirmed.state = state;
    }
    if update.priority.is_some() {
        confirmed.priority = priority;
    }
    if update.assignee_id.is_some() {
        confirmed.assignee = assignee;
    }
    if update.label_ids.is_some()
        || !update.added_label_ids.is_empty()
        || !update.removed_label_ids.is_empty()
    {
        confirmed.labels = labels;
    }
    if update.estimate.is_some() {
        confirmed.estimate = estimate;
    }
    if update.due_date.is_some() {
        confirmed.due_date = due_date;
    }
    if update.project_id.is_some() {
        confirmed.project = project;
    }
    if update.cycle_id.is_some() {
        confirmed.cycle = cycle;
    }
    if update.title.is_some() {
        confirmed.title = title;
    }
    if update.sort_order.is_some() {
        confirmed.sort_order = sort_order;
    }
    confirmed.updated_at = confirmed.updated_at.max(updated_at);
}

#[cfg(test)]
mod tests {
    use super::OptimisticIssue;
    use chrono::{DateTime, Utc};
    use loupe_linear::{
        IssueSummary, IssueUpdate, Label, Priority, StateRef, StateType, Team, TeamRef, User,
        WorkflowState,
    };

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().to_utc()
    }

    fn user(id: &str) -> User {
        User {
            id: id.into(),
            name: format!("User {id}"),
            display_name: id.into(),
            avatar_url: None,
        }
    }

    fn label(id: &str) -> Label {
        Label {
            id: id.into(),
            name: id.into(),
            color: "#ffffff".into(),
            parent_id: None,
        }
    }

    fn workflow_state(id: &str, kind: StateType) -> WorkflowState {
        WorkflowState {
            id: id.into(),
            name: id.into(),
            kind,
            position: 0.0,
            color: "#000000".into(),
        }
    }

    fn state_ref(id: &str, kind: StateType) -> StateRef {
        StateRef {
            id: id.into(),
            name: id.into(),
            kind,
            color: "#000000".into(),
        }
    }

    fn team() -> Team {
        Team {
            id: "t1".into(),
            key: "ENG".into(),
            name: "Engineering".into(),
            states: vec![
                workflow_state("todo", StateType::Unstarted),
                workflow_state("doing", StateType::Started),
                workflow_state("done", StateType::Completed),
            ],
            members: vec![user("ana"), user("bob")],
            labels: vec![label("bug"), label("ui"), label("api")],
            projects: Vec::new(),
            cycles: Vec::new(),
            label_groups: Vec::new(),
            estimation: Default::default(),
        }
    }

    fn issue() -> IssueSummary {
        IssueSummary {
            id: "i1".into(),
            identifier: "ENG-1".into(),
            title: "Fix it".into(),
            priority: Priority::Low,
            sort_order: 0.,
            state: state_ref("todo", StateType::Unstarted),
            team: TeamRef {
                id: "t1".into(),
                key: "ENG".into(),
                name: "Engineering".into(),
            },
            assignee: None,
            labels: vec![label("bug")],
            project: None,
            cycle: None,
            estimate: None,
            due_date: None,
            url: "https://linear.app/x/issue/ENG-1".into(),
            branch_name: String::new(),
            created_at: at("2026-11-01T00:00:00Z"),
            updated_at: at("2026-11-01T00:00:00Z"),
            creator_id: None,
            parent_id: None,
            has_sub_issues: false,
            milestone: None,
            closed_at: None,
            pull_requests: Vec::new(),
            pull_requests_complete: true,
        }
    }

    fn priority(priority: Priority) -> IssueUpdate {
        IssueUpdate {
            priority: Some(priority),
            ..IssueUpdate::default()
        }
    }

    fn state(id: &str) -> IssueUpdate {
        IssueUpdate {
            state_id: Some(id.into()),
            ..IssueUpdate::default()
        }
    }

    fn assignee(id: Option<&str>) -> IssueUpdate {
        IssueUpdate {
            assignee_id: Some(id.map(str::to_owned)),
            ..IssueUpdate::default()
        }
    }

    fn labels(ids: &[&str]) -> IssueUpdate {
        IssueUpdate {
            label_ids: Some(ids.iter().map(|id| (*id).to_owned()).collect()),
            ..IssueUpdate::default()
        }
    }

    /// What the server returns after applying `edits` to the stored issue.
    fn server_after(edits: &[IssueUpdate], team: &Team) -> IssueSummary {
        let mut model = OptimisticIssue::new(issue());
        for edit in edits {
            model.begin(edit.clone());
        }
        let mut server = model.displayed(team);
        server.updated_at = at("2026-11-02T00:00:00Z");
        server
    }

    #[test]
    fn a_single_edit_shows_at_once() {
        let team = team();
        let mut model = OptimisticIssue::new(issue());
        model.begin(priority(Priority::Urgent));
        assert_eq!(model.displayed(&team).priority, Priority::Urgent);
        assert_eq!(model.confirmed().priority, Priority::Low);
        assert!(model.has_pending());
    }

    #[test]
    fn each_field_maps_through_the_team_catalog() {
        let team = team();
        let mut model = OptimisticIssue::new(issue());
        model.begin(state("doing"));
        model.begin(assignee(Some("bob")));
        model.begin(labels(&["ui", "bug"]));
        let shown = model.displayed(&team);
        assert_eq!(shown.state.id, "doing");
        assert_eq!(shown.state.kind, StateType::Started);
        assert_eq!(shown.assignee.map(|user| user.id), Some("bob".to_owned()));
        let ids: Vec<_> = shown.labels.iter().map(|label| label.id.as_str()).collect();
        assert_eq!(ids, ["ui", "bug"]);
    }

    #[test]
    fn clearing_the_assignee_shows_unassigned() {
        let team = team();
        let mut confirmed = issue();
        confirmed.assignee = Some(user("ana"));
        let mut model = OptimisticIssue::new(confirmed);
        model.begin(assignee(None));
        assert_eq!(model.displayed(&team).assignee, None);
    }

    #[test]
    fn a_failed_edit_rolls_back_to_the_previous_value() {
        let team = team();
        let mut model = OptimisticIssue::new(issue());
        let seq = model.begin(priority(Priority::Urgent));
        model.fail(seq);
        assert_eq!(model.displayed(&team), issue());
        assert!(!model.has_pending());
    }

    #[test]
    fn a_confirmed_edit_becomes_the_confirmed_value() {
        let team = team();
        let mut model = OptimisticIssue::new(issue());
        let seq = model.begin(priority(Priority::Urgent));
        model.take_next();
        model.confirm(seq, server_after(&[priority(Priority::Urgent)], &team));
        assert_eq!(model.confirmed().priority, Priority::Urgent);
        assert_eq!(model.displayed(&team).priority, Priority::Urgent);
        assert_eq!(model.confirmed().updated_at, at("2026-11-02T00:00:00Z"));
        assert!(!model.has_pending());
    }

    #[test]
    fn first_fails_second_succeeds_leaves_only_the_second() {
        let team = team();
        let mut model = OptimisticIssue::new(issue());
        let first = model.begin(priority(Priority::Urgent));
        let second = model.begin(state("done"));
        model.fail(first);
        let shown = model.displayed(&team);
        assert_eq!(shown.priority, Priority::Low);
        assert_eq!(shown.state.id, "done");

        model.take_next();
        model.confirm(second, server_after(&[state("done")], &team));
        let shown = model.displayed(&team);
        assert_eq!(shown.priority, Priority::Low);
        assert_eq!(shown.state.id, "done");
        assert!(!model.has_pending());
    }

    #[test]
    fn first_succeeds_second_fails_leaves_only_the_first() {
        let team = team();
        let mut model = OptimisticIssue::new(issue());
        let first = model.begin(priority(Priority::Urgent));
        let second = model.begin(state("done"));
        model.take_next();
        model.confirm(first, server_after(&[priority(Priority::Urgent)], &team));
        assert_eq!(model.displayed(&team).state.id, "done");
        model.fail(second);
        let shown = model.displayed(&team);
        assert_eq!(shown.priority, Priority::Urgent);
        assert_eq!(shown.state.id, "todo");
    }

    #[test]
    fn edits_to_different_fields_both_end_confirmed() {
        let team = team();
        let edits = [priority(Priority::High), assignee(Some("ana"))];
        // Linear applies the edits in the order sent: the first response knows
        // only the first edit, the second knows both.
        let after_first = server_after(&edits[..1], &team);
        let after_both = server_after(&edits, &team);

        let mut model = OptimisticIssue::new(issue());
        let first = model.begin(edits[0].clone());
        let second = model.begin(edits[1].clone());
        model.take_next();
        model.confirm(first, after_first);
        model.take_next();
        model.confirm(second, after_both);
        let shown = model.displayed(&team);
        assert_eq!(shown.priority, Priority::High);
        assert_eq!(shown.assignee.map(|user| user.id), Some("ana".to_owned()));
        assert!(!model.has_pending());
    }

    #[test]
    fn an_answer_for_an_unsent_edit_is_ignored() {
        let team = team();
        let mut model = OptimisticIssue::new(issue());
        model.begin(priority(Priority::High));
        let second = model.begin(priority(Priority::Urgent));
        model.take_next();
        model.confirm(second, server_after(&[priority(Priority::Urgent)], &team));
        assert_eq!(model.confirmed().priority, Priority::Low);
        assert_eq!(model.displayed(&team).priority, Priority::Urgent);
    }

    #[test]
    fn a_later_edit_to_the_same_field_stays_on_top_of_an_earlier_confirm() {
        let team = team();
        let mut model = OptimisticIssue::new(issue());
        let first = model.begin(priority(Priority::High));
        model.begin(priority(Priority::Urgent));
        model.take_next();
        model.confirm(first, server_after(&[priority(Priority::High)], &team));
        assert_eq!(model.confirmed().priority, Priority::High);
        assert_eq!(model.displayed(&team).priority, Priority::Urgent);
    }

    #[test]
    fn a_failed_edit_to_a_field_keeps_a_later_edit_to_that_field() {
        let team = team();
        let mut model = OptimisticIssue::new(issue());
        let first = model.begin(priority(Priority::High));
        model.begin(priority(Priority::Urgent));
        model.fail(first);
        assert_eq!(model.displayed(&team).priority, Priority::Urgent);
    }

    #[test]
    fn a_failed_label_edit_is_not_resent_by_a_later_one() {
        let team = team();
        let mut model = OptimisticIssue::new(issue());
        let add_ui = model.begin_label("ui".into(), true);
        let add_api = model.begin_label("api".into(), true);
        let ids = |model: &OptimisticIssue| -> Vec<String> {
            model
                .displayed(&team)
                .labels
                .into_iter()
                .map(|label| label.id)
                .collect()
        };
        assert_eq!(ids(&model), ["bug", "ui", "api"]);

        let (seq, first) = model.take_next().expect("first toggle");
        assert_eq!(seq, add_ui);
        assert_eq!(
            (first.label_ids, first.added_label_ids),
            (None, vec!["ui".to_owned()]),
            "one label added, the others left as Linear holds them"
        );
        model.fail(add_ui);
        assert_eq!(ids(&model), ["bug", "api"]);

        let (seq, second) = model.take_next().expect("second toggle");
        assert_eq!(seq, add_api);
        assert_eq!(
            (second.label_ids, second.added_label_ids),
            (None, vec!["api".to_owned()]),
            "the failed ui label must not ride along"
        );
    }

    #[test]
    fn checking_then_unchecking_a_label_ends_unchecked_even_if_the_first_fails() {
        let team = team();
        let mut model = OptimisticIssue::new(issue());
        let check = model.begin_label("ui".into(), true);
        let uncheck = model.begin_label("ui".into(), false);
        let has_ui = |model: &OptimisticIssue| {
            model
                .displayed(&team)
                .labels
                .iter()
                .any(|label| label.id == "ui")
        };
        assert!(!has_ui(&model));

        model.take_next();
        model.fail(check);
        let (seq, update) = model.take_next().expect("the uncheck is next");
        assert_eq!(seq, uncheck);
        assert_eq!(
            (update.label_ids, update.removed_label_ids),
            (None, vec!["ui".to_owned()])
        );
        assert!(!has_ui(&model));
    }

    #[test]
    fn edits_are_sent_one_at_a_time_in_order() {
        let team = team();
        let mut model = OptimisticIssue::new(issue());
        let first = model.begin(priority(Priority::High));
        let second = model.begin(priority(Priority::Urgent));
        assert_eq!(model.take_next().map(|(seq, _)| seq), Some(first));
        assert_eq!(model.take_next(), None, "the first is still in flight");
        assert_eq!(model.displayed(&team).priority, Priority::Urgent);

        model.confirm(first, server_after(&[priority(Priority::High)], &team));
        let (seq, update) = model.take_next().expect("the second is next");
        assert_eq!(seq, second);
        assert_eq!(update.priority, Some(Priority::Urgent));
        model.fail(second);
        assert_eq!(model.take_next(), None);
        assert_eq!(model.displayed(&team).priority, Priority::High);
    }

    #[test]
    fn a_failed_edit_lets_the_next_one_go() {
        let mut model = OptimisticIssue::new(issue());
        let first = model.begin(priority(Priority::High));
        let second = model.begin(state("done"));
        model.take_next();
        model.fail(first);
        assert_eq!(model.take_next().map(|(seq, _)| seq), Some(second));
    }

    #[test]
    fn answers_for_unknown_edits_change_nothing() {
        let team = team();
        let mut model = OptimisticIssue::new(issue());
        model.confirm(7, server_after(&[priority(Priority::Urgent)], &team));
        model.fail(7);
        assert_eq!(model.displayed(&team), issue());
    }

    #[test]
    fn an_id_missing_from_the_catalog_keeps_the_confirmed_value() {
        let team = team();
        let mut model = OptimisticIssue::new(issue());
        model.begin(state("missing"));
        model.begin(assignee(Some("ghost")));
        let shown = model.displayed(&team);
        assert_eq!(shown.state.id, "todo");
        assert_eq!(shown.assignee, None);
    }
}
