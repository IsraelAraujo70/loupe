//! What the run page lists, without GPUI: the jobs column grouped by matrix
//! and ordered with what runs first, then what fails, then the rest; and the
//! annotations grouped by job with each message once.

use loupe_github::{Annotation, AnnotationLevel, CheckStatus};

use super::layout::{GraphJob, GraphNode};

/// Which jobs the column shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JobFilter {
    #[default]
    All,
    Failing,
    Running,
}

impl JobFilter {
    pub const ALL: [JobFilter; 3] = [JobFilter::All, JobFilter::Failing, JobFilter::Running];

    pub fn label(self) -> &'static str {
        match self {
            JobFilter::All => "All",
            JobFilter::Failing => "Failing",
            JobFilter::Running => "Running",
        }
    }

    fn keeps(self, status: CheckStatus) -> bool {
        match self {
            JobFilter::All => true,
            JobFilter::Failing => Phase::of(status) == Phase::Failing,
            JobFilter::Running => Phase::of(status) == Phase::Running,
        }
    }
}

/// Where a job goes in the column: what still runs first, then what failed,
/// then everything that ended otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Running,
    Failing,
    Rest,
}

impl Phase {
    fn of(status: CheckStatus) -> Self {
        match status {
            CheckStatus::Running | CheckStatus::Queued | CheckStatus::ActionRequired => {
                Phase::Running
            }
            CheckStatus::Failed | CheckStatus::Cancelled => Phase::Failing,
            CheckStatus::Passed | CheckStatus::Neutral | CheckStatus::Skipped => Phase::Rest,
        }
    }
}

/// A job's place: its phase, and a running job before a queued one.
fn job_key(job: &GraphJob) -> (Phase, bool) {
    (Phase::of(job.status), job.status != CheckStatus::Running)
}

/// One entry of the jobs column: a job alone (`name` is `None`), or the jobs
/// of one matrix or one called workflow under its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobGroup {
    pub name: Option<String>,
    pub jobs: Vec<GraphJob>,
    /// Of the whole group, whatever the filter: its state as one
    /// ([`group_status`]), how many jobs ended, and how many it has.
    pub status: CheckStatus,
    pub completed: usize,
    pub total: usize,
}

/// The jobs column for `filter`: the graph's boxes as groups, each group's
/// jobs ordered running, failing, then the rest (in the graph's order
/// otherwise), and the groups ordered by their first job the same way. A
/// group the filter empties is left out.
pub fn job_groups(nodes: &[GraphNode], filter: JobFilter) -> Vec<JobGroup> {
    let mut groups: Vec<JobGroup> = nodes
        .iter()
        .filter_map(|node| {
            let mut jobs: Vec<GraphJob> = node
                .jobs
                .iter()
                .filter(|job| filter.keeps(job.status))
                .cloned()
                .collect();
            if jobs.is_empty() {
                return None;
            }
            jobs.sort_by_key(job_key);
            Some(JobGroup {
                name: node.group_name().map(str::to_owned),
                jobs,
                status: group_status(&node.jobs),
                completed: node.completed(),
                total: node.jobs.len(),
            })
        })
        .collect();
    groups.sort_by_key(|group| group.jobs.first().map(job_key));
    groups
}

/// The state a group of jobs shows as one: running while any job runs, else
/// failed while any failed, else cancelled while any was, else passed when
/// one passed, else skipped.
pub fn group_status(jobs: &[GraphJob]) -> CheckStatus {
    let any = |status: CheckStatus| jobs.iter().any(|job| job.status == status);
    [
        CheckStatus::Running,
        CheckStatus::Queued,
        CheckStatus::ActionRequired,
        CheckStatus::Failed,
        CheckStatus::Cancelled,
        CheckStatus::Passed,
        CheckStatus::Neutral,
    ]
    .into_iter()
    .find(|status| any(*status))
    .unwrap_or(CheckStatus::Skipped)
}

/// One message of one job, and how many times the job left it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnotationEntry {
    pub annotation: Annotation,
    pub times: usize,
}

/// The annotations one job left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnotationGroup {
    pub job: String,
    pub job_id: Option<u64>,
    pub entries: Vec<AnnotationEntry>,
}

impl AnnotationGroup {
    pub fn count(&self, level: AnnotationLevel) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.annotation.level == level)
            .count()
    }
}

/// The annotations by job: a job's group holds each message once (the same
/// level, title, text, and place), counted, failures first; the groups go
/// in the order of their worst message, then in the order GitHub gave them.
pub fn annotation_groups(annotations: &[Annotation]) -> Vec<AnnotationGroup> {
    let mut groups: Vec<AnnotationGroup> = Vec::new();
    for annotation in annotations {
        let ix = match groups
            .iter()
            .position(|group| group.job_id == annotation.job_id && group.job == annotation.job)
        {
            Some(ix) => ix,
            None => {
                groups.push(AnnotationGroup {
                    job: annotation.job.clone(),
                    job_id: annotation.job_id,
                    entries: Vec::new(),
                });
                groups.len() - 1
            }
        };
        let entries = &mut groups[ix].entries;
        let same = |entry: &&mut AnnotationEntry| {
            let seen = &entry.annotation;
            seen.level == annotation.level
                && seen.title == annotation.title
                && seen.message == annotation.message
                && seen.path == annotation.path
                && seen.line == annotation.line
        };
        match entries.iter_mut().find(same) {
            Some(entry) => entry.times += 1,
            None => entries.push(AnnotationEntry {
                annotation: annotation.clone(),
                times: 1,
            }),
        }
    }
    for group in &mut groups {
        group
            .entries
            .sort_by_key(|entry| entry.annotation.level as u8);
    }
    groups.sort_by_key(|group| {
        group
            .entries
            .first()
            .map_or(u8::MAX, |entry| entry.annotation.level as u8)
    });
    groups
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use loupe_github::{Annotation, AnnotationLevel, CheckStatus, WorkflowJob, parse_workflow};

    use super::{JobFilter, annotation_groups, group_status, job_groups};
    use crate::github::actions::layout::graph_nodes;

    const WORKFLOW: &str = "jobs:
  version:
    runs-on: x
  test:
    needs: version
    strategy:
      matrix:
        os: [linux, mac, windows]
  lint:
    needs: version
  publish:
    needs: [test, lint]
";

    fn job(id: u64, name: &str, status: CheckStatus) -> WorkflowJob {
        let at: DateTime<Utc> = "2026-11-11T10:00:00Z".parse().expect("start");
        WorkflowJob {
            id,
            name: name.into(),
            status,
            started_at: Some(at),
            completed_at: Some(at),
            url: None,
            runner: None,
            steps: Vec::new(),
        }
    }

    fn groups(jobs: &[WorkflowJob], filter: JobFilter) -> Vec<(Option<String>, Vec<String>)> {
        let now = "2026-11-11T11:00:00Z".parse().expect("now");
        let nodes = graph_nodes(Some(&parse_workflow(WORKFLOW)), jobs, now);
        job_groups(&nodes, filter)
            .into_iter()
            .map(|group| {
                (
                    group.name,
                    group.jobs.into_iter().map(|job| job.name).collect(),
                )
            })
            .collect()
    }

    fn run() -> Vec<WorkflowJob> {
        vec![
            job(1, "version", CheckStatus::Passed),
            job(2, "test (linux)", CheckStatus::Passed),
            job(3, "test (mac)", CheckStatus::Failed),
            job(4, "test (windows)", CheckStatus::Running),
            job(5, "lint", CheckStatus::Failed),
            job(6, "publish", CheckStatus::Queued),
        ]
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_matrix_is_one_group_and_running_comes_before_failing_before_the_rest() {
        assert_eq!(
            groups(&run(), JobFilter::All),
            [
                // The matrix leads: a job of it runs. Inside it the running
                // job, then the failed one, then the one that passed.
                (
                    Some("test".to_owned()),
                    names(&["test (windows)", "test (mac)", "test (linux)"])
                ),
                // Queued after running, still before what failed.
                (None, names(&["publish"])),
                (None, names(&["lint"])),
                (None, names(&["version"])),
            ]
        );
    }

    #[test]
    fn the_filters_keep_failing_or_running_jobs_in_their_groups() {
        assert_eq!(
            groups(&run(), JobFilter::Failing),
            [
                (Some("test".to_owned()), names(&["test (mac)"])),
                (None, names(&["lint"])),
            ]
        );
        assert_eq!(
            groups(&run(), JobFilter::Running),
            [
                (Some("test".to_owned()), names(&["test (windows)"])),
                (None, names(&["publish"])),
            ]
        );
    }

    #[test]
    fn a_group_counts_its_ended_jobs_whatever_the_filter() {
        let now = "2026-11-11T11:00:00Z".parse().expect("now");
        let nodes = graph_nodes(Some(&parse_workflow(WORKFLOW)), &run(), now);
        let matrix = job_groups(&nodes, JobFilter::Failing)
            .into_iter()
            .find(|group| group.name.is_some())
            .expect("matrix");
        assert_eq!((matrix.completed, matrix.total), (2, 3));
        // Only its failed job shows, yet the matrix still runs.
        assert_eq!(matrix.status, CheckStatus::Running);
    }

    #[test]
    fn a_group_reads_as_its_most_urgent_job() {
        let now = "2026-11-11T11:00:00Z".parse().expect("now");
        let nodes = graph_nodes(Some(&parse_workflow(WORKFLOW)), &run(), now);
        let status = |ix: usize| group_status(&nodes[ix].jobs);
        assert_eq!(status(1), CheckStatus::Running);
        let ended = [
            job(2, "test (linux)", CheckStatus::Passed),
            job(3, "test (mac)", CheckStatus::Skipped),
            job(4, "test (windows)", CheckStatus::Failed),
        ];
        let nodes = graph_nodes(Some(&parse_workflow(WORKFLOW)), &ended, now);
        assert_eq!(group_status(&nodes[0].jobs), CheckStatus::Failed);
    }

    fn annotation(job: &str, id: u64, level: AnnotationLevel, message: &str) -> Annotation {
        Annotation {
            job: job.into(),
            job_id: Some(id),
            level,
            title: None,
            message: message.into(),
            path: ".github".into(),
            line: None,
            url: String::new(),
        }
    }

    /// A message's level, its text, and how many times it came.
    type Entry = (AnnotationLevel, String, usize);

    #[test]
    fn annotations_group_by_job_and_a_repeated_message_shows_once() {
        let deprecated = "Node.js 20 actions are deprecated.";
        let list = [
            annotation("lint", 5, AnnotationLevel::Warning, deprecated),
            annotation("lint", 5, AnnotationLevel::Warning, deprecated),
            annotation("test (mac)", 3, AnnotationLevel::Warning, deprecated),
            annotation("test (mac)", 3, AnnotationLevel::Failure, "exit code 101"),
            annotation("lint", 5, AnnotationLevel::Warning, deprecated),
        ];
        let groups = annotation_groups(&list);
        let shape: Vec<(String, Vec<Entry>)> = groups
            .iter()
            .map(|group| {
                (
                    group.job.clone(),
                    group
                        .entries
                        .iter()
                        .map(|entry| {
                            (
                                entry.annotation.level,
                                entry.annotation.message.clone(),
                                entry.times,
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (
                    "test (mac)".to_owned(),
                    vec![
                        (AnnotationLevel::Failure, "exit code 101".to_owned(), 1),
                        (AnnotationLevel::Warning, deprecated.to_owned(), 1),
                    ]
                ),
                (
                    "lint".to_owned(),
                    vec![(AnnotationLevel::Warning, deprecated.to_owned(), 3)]
                ),
            ]
        );
        assert_eq!(groups[0].count(AnnotationLevel::Failure), 1);
    }
}
