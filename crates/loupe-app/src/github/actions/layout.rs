//! The run graph's shape, without GPUI: which jobs share a box (a matrix, a
//! called workflow), which boxes need which, and where each box goes when
//! the graph runs from left to right, one column per step of `needs`. Sizes
//! are at 100% zoom.

use chrono::{DateTime, Utc};
use loupe_github::{CheckStatus, WorkflowJob, WorkflowSpec, assign_jobs, called_job_name};

pub const NODE_WIDTH: f32 = 240.;
/// One job's row, alone or inside a box.
pub const JOB_HEIGHT: f32 = 32.;
/// A box's title over its jobs.
pub const TITLE_HEIGHT: f32 = 24.;
/// Room under the last job of a box.
pub const BOX_PADDING: f32 = 6.;
/// Between two columns, where the lines run.
const COLUMN_GAP: f32 = 64.;
/// Between two boxes of one column.
const ROW_GAP: f32 = 16.;
/// Without a workflow file there are no edges; the jobs wrap at this many
/// per column instead of standing in one long column.
const GRID_ROWS: usize = 4;

/// One job as the graph shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphJob {
    pub id: u64,
    /// Its name inside its box: a called workflow's job drops the caller.
    pub label: String,
    pub name: String,
    pub status: CheckStatus,
    pub seconds: Option<i64>,
}

/// What a box of several jobs stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
    /// A job of the file that fans out over a matrix: its box can fold to a
    /// line that counts its jobs.
    Matrix,
    /// A job that calls another workflow: its box holds that workflow's jobs.
    Called,
}

/// A box of the graph: one job, or the jobs of one matrix or one called
/// workflow under a title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphNode {
    /// The matrix's key or the called workflow's name; `None` for one job.
    pub group: Option<(GroupKind, String)>,
    pub jobs: Vec<GraphJob>,
    /// The boxes it needs, by index.
    pub needs: Vec<usize>,
}

impl GraphNode {
    /// The box's title: `Matrix: <key>`, or the called workflow's name.
    pub fn title(&self) -> Option<String> {
        self.group.as_ref().map(|(kind, name)| match kind {
            GroupKind::Matrix => format!("Matrix: {name}"),
            GroupKind::Called => name.clone(),
        })
    }

    /// What the jobs column calls the group, `None` for one job.
    pub fn group_name(&self) -> Option<&str> {
        self.group.as_ref().map(|(_, name)| name.as_str())
    }

    /// A matrix of more than one job folds.
    pub fn foldable(&self) -> bool {
        matches!(self.group, Some((GroupKind::Matrix, _))) && self.jobs.len() > 1
    }

    /// How many of its jobs have ended.
    pub fn completed(&self) -> usize {
        self.jobs
            .iter()
            .filter(|job| {
                !matches!(
                    job.status,
                    CheckStatus::Queued | CheckStatus::Running | CheckStatus::ActionRequired
                )
            })
            .count()
    }

    pub fn contains(&self, job: u64) -> bool {
        self.jobs.iter().any(|known| known.id == job)
    }

    /// Its height, its jobs shown when `open` (a box that does not fold is
    /// always open); folded, a box is its title over one line.
    pub fn height(&self, open: bool) -> f32 {
        let rows = if open || !self.foldable() {
            self.jobs.len()
        } else {
            1
        };
        let rows = rows as f32 * JOB_HEIGHT;
        match self.group {
            Some(_) => TITLE_HEIGHT + rows + BOX_PADDING,
            None => rows,
        }
    }
}

/// The boxes of a run: one per job of the workflow file that ran, in the
/// file's order, with what it needs; a job the file does not explain stands
/// alone.
pub fn graph_nodes(
    spec: Option<&WorkflowSpec>,
    jobs: &[WorkflowJob],
    now: DateTime<Utc>,
) -> Vec<GraphNode> {
    let names: Vec<&str> = jobs.iter().map(|job| job.name.as_str()).collect();
    let empty = WorkflowSpec::default();
    let spec = spec.unwrap_or(&empty);
    let assigned = assign_jobs(spec, &names);
    let graph_job = |job: &WorkflowJob, label: &str| GraphJob {
        id: job.id,
        label: label.to_owned(),
        name: job.name.clone(),
        status: job.status,
        seconds: job.duration_seconds(now),
    };
    let mut nodes: Vec<GraphNode> = Vec::new();
    // The box of each job of the file that ran.
    let mut node_of_spec: Vec<Option<usize>> = vec![None; spec.jobs.len()];
    for (spec_ix, spec_job) in spec.jobs.iter().enumerate() {
        let mut members: Vec<GraphJob> = jobs
            .iter()
            .zip(&assigned)
            .filter(|(_, assigned)| **assigned == Some(spec_ix))
            .map(|(job, _)| graph_job(job, called_job_name(spec_job, &job.name)))
            .collect();
        // GitHub lists a matrix's jobs as they start; `tests 2` goes
        // before `tests 10`.
        members.sort_by_key(|job| natural_key(&job.label));
        if members.is_empty() {
            continue;
        }
        let unexpanded = members.len() == 1 && members[0].name == spec_job.display_name();
        let group = match () {
            _ if unexpanded => None,
            _ if spec_job.reusable => Some((GroupKind::Called, spec_job.display_name().to_owned())),
            _ if spec_job.matrix || members.len() > 1 => {
                Some((GroupKind::Matrix, spec_job.key.clone()))
            }
            _ => None,
        };
        node_of_spec[spec_ix] = Some(nodes.len());
        nodes.push(GraphNode {
            group,
            jobs: members,
            needs: Vec::new(),
        });
    }
    for (spec_ix, spec_job) in spec.jobs.iter().enumerate() {
        let Some(node) = node_of_spec[spec_ix] else {
            continue;
        };
        let mut needs: Vec<usize> = spec_job
            .needs
            .iter()
            .filter_map(|key| spec.jobs.iter().position(|job| &job.key == key))
            .filter_map(|ix| node_of_spec[ix])
            .filter(|need| *need != node)
            .collect();
        needs.sort_unstable();
        needs.dedup();
        nodes[node].needs = needs;
    }
    for (job, assigned) in jobs.iter().zip(&assigned) {
        if assigned.is_none() {
            nodes.push(GraphNode {
                group: None,
                jobs: vec![graph_job(job, &job.name)],
                needs: Vec::new(),
            });
        }
    }
    nodes
}

/// A label split into text and numbers, so numbers compare by value.
fn natural_key(label: &str) -> Vec<(String, u64)> {
    let mut key: Vec<(String, u64)> = Vec::new();
    let mut text = String::new();
    let mut digits = String::new();
    for c in label.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            if !digits.is_empty() {
                key.push((
                    std::mem::take(&mut text),
                    digits.parse().unwrap_or(u64::MAX),
                ));
                digits.clear();
            }
            text.push(c);
        }
    }
    let number = if digits.is_empty() {
        0
    } else {
        digits.parse().unwrap_or(u64::MAX)
    };
    key.push((text, number));
    key
}

/// Where a box sits, at 100% zoom, from the graph's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Placed {
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn center_y(&self) -> f32 {
        self.y + self.height / 2.
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub boxes: Vec<Placed>,
    /// `(from, to)`: `to` needs `from`.
    pub edges: Vec<(usize, usize)>,
    pub width: f32,
    pub height: f32,
}

/// The ends and control points of an edge's curve: it leaves the right side
/// of the box it comes from and enters the left side of the one that needs
/// it, both level, so the line is a smooth S.
pub fn edge_curve(from: &Placed, to: &Placed) -> [(f32, f32); 4] {
    let start = (from.right(), from.center_y());
    let end = (to.x, to.center_y());
    let bend = ((end.0 - start.0) / 2.).max(24.);
    [start, (start.0 + bend, start.1), (end.0 - bend, end.1), end]
}

/// Lays the boxes out from left to right: each box one column after the
/// rightmost box it needs, columns ordered to keep a box level with what it
/// needs, and each box centered on its parents. `open[ix]` says whether box
/// `ix` shows its jobs (see [`GraphNode::height`]).
pub fn layout(nodes: &[GraphNode], open: &[bool]) -> Layout {
    let count = nodes.len();
    if count == 0 {
        return Layout::default();
    }
    let height = |ix: usize| nodes[ix].height(open.get(ix).copied().unwrap_or(true));
    let edges: Vec<(usize, usize)> = nodes
        .iter()
        .enumerate()
        .flat_map(|(to, node)| {
            node.needs
                .iter()
                .filter(move |from| **from < count)
                .map(move |from| (*from, to))
        })
        .collect();
    let rank = if edges.is_empty() {
        (0..count).map(|ix| ix / GRID_ROWS).collect()
    } else {
        ranks(nodes)
    };
    let depth = rank.iter().copied().max().unwrap_or(0) + 1;
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); depth];
    for (ix, rank) in rank.iter().enumerate() {
        columns[*rank].push(ix);
    }
    order_rows(&mut columns, nodes);

    let mut boxes = vec![
        Placed {
            x: 0.,
            y: 0.,
            width: NODE_WIDTH,
            height: 0.,
        };
        count
    ];
    for (column_ix, column) in columns.iter().enumerate() {
        let x = column_ix as f32 * (NODE_WIDTH + COLUMN_GAP);
        // Where each box would like its top: level with its parents' middle,
        // or stacked under the box before it.
        let mut stacked = 0.;
        let wanted: Vec<f32> = column
            .iter()
            .map(|ix| {
                let parents: Vec<f32> = nodes[*ix]
                    .needs
                    .iter()
                    .filter(|need| rank[**need] < rank[*ix])
                    .map(|need| boxes[*need].center_y())
                    .collect();
                let top = if parents.is_empty() || column_ix == 0 {
                    stacked
                } else {
                    parents.iter().sum::<f32>() / parents.len() as f32 - height(*ix) / 2.
                };
                stacked += height(*ix) + ROW_GAP;
                top
            })
            .collect();
        let mut tops: Vec<f32> = Vec::with_capacity(column.len());
        for (slot, want) in wanted.iter().enumerate() {
            let floor = match slot {
                0 => f32::MIN,
                _ => tops[slot - 1] + height(column[slot - 1]) + ROW_GAP,
            };
            tops.push(want.max(floor));
        }
        // Pushing down drifts the column; slide it back so it sits as close
        // to where it wanted to be on average.
        let drift = wanted.iter().zip(&tops).map(|(w, t)| t - w).sum::<f32>() / column.len() as f32;
        for (slot, ix) in column.iter().enumerate() {
            boxes[*ix] = Placed {
                x,
                y: tops[slot] - drift,
                width: NODE_WIDTH,
                height: height(*ix),
            };
        }
    }
    let top = boxes.iter().map(|b| b.y).fold(f32::MAX, f32::min);
    for placed in &mut boxes {
        placed.y -= top;
    }
    let width = boxes.iter().map(Placed::right).fold(0., f32::max);
    let height = boxes.iter().map(Placed::bottom).fold(0., f32::max);
    Layout {
        boxes,
        edges,
        width,
        height,
    }
}

/// Each box's column: one after the rightmost box it needs. A cycle (which GitHub
/// rejects) cannot loop forever: ranks stop growing after `count` passes.
fn ranks(nodes: &[GraphNode]) -> Vec<usize> {
    let count = nodes.len();
    let mut rank = vec![0usize; count];
    for _ in 0..count {
        let mut changed = false;
        for (ix, node) in nodes.iter().enumerate() {
            let wanted = node
                .needs
                .iter()
                .filter(|need| **need < count)
                .map(|need| rank[*need] + 1)
                .max()
                .unwrap_or(0);
            if wanted > rank[ix] {
                rank[ix] = wanted.min(count);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    rank
}

/// Orders each column by the mean slot of the boxes it needs (going right)
/// and then of the boxes that need it (going left), a few times, which keeps
/// edges from crossing where it can.
fn order_rows(rows: &mut [Vec<usize>], nodes: &[GraphNode]) {
    let count = nodes.len();
    let mut slot = vec![0f32; count];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (ix, node) in nodes.iter().enumerate() {
        for need in &node.needs {
            if *need < count {
                children[*need].push(ix);
            }
        }
    }
    let place = |rows: &[Vec<usize>], slot: &mut [f32]| {
        for row in rows {
            for (position, ix) in row.iter().enumerate() {
                slot[*ix] = position as f32;
            }
        }
    };
    place(rows, &mut slot);
    let mean = |of: &[usize], slot: &[f32]| -> Option<f32> {
        (!of.is_empty()).then(|| of.iter().map(|ix| slot[*ix]).sum::<f32>() / of.len() as f32)
    };
    for _ in 0..4 {
        for depth in 1..rows.len() {
            let keys: Vec<f32> = rows[depth]
                .iter()
                .map(|ix| mean(&nodes[*ix].needs, &slot).unwrap_or(slot[*ix]))
                .collect();
            sort_by_keys(&mut rows[depth], &keys);
            place(rows, &mut slot);
        }
        for depth in (0..rows.len().saturating_sub(1)).rev() {
            let keys: Vec<f32> = rows[depth]
                .iter()
                .map(|ix| mean(&children[*ix], &slot).unwrap_or(slot[*ix]))
                .collect();
            sort_by_keys(&mut rows[depth], &keys);
            place(rows, &mut slot);
        }
    }
}

fn sort_by_keys(row: &mut Vec<usize>, keys: &[f32]) {
    let mut keyed: Vec<(f32, usize, usize)> = row
        .iter()
        .zip(keys)
        .enumerate()
        .map(|(position, (ix, key))| (*key, position, *ix))
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    *row = keyed.into_iter().map(|(_, _, ix)| ix).collect();
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use loupe_github::{CheckStatus, WorkflowJob, parse_workflow};

    use super::{
        BOX_PADDING, GraphNode, JOB_HEIGHT, NODE_WIDTH, TITLE_HEIGHT, edge_curve, graph_nodes,
        layout,
    };

    const WORKFLOW: &str = "jobs:
  version:
    runs-on: x
  test:
    name: Tests ${{ matrix.shard }}
    needs: version
    strategy:
      matrix:
        shard: [1, 2, 3]
  desktop:
    name: Desktop Linux
    needs: [version]
    uses: ./.github/workflows/desktop.yml
  publish:
    needs: [test, desktop]
";

    fn job(id: u64, name: &str) -> WorkflowJob {
        let at: DateTime<Utc> = "2026-11-11T10:00:00Z".parse().expect("start");
        WorkflowJob {
            id,
            name: name.into(),
            status: CheckStatus::Passed,
            started_at: Some(at),
            completed_at: Some(at + chrono::Duration::seconds(id as i64 * 10)),
            url: None,
            runner: None,
            steps: Vec::new(),
        }
    }

    fn jobs() -> Vec<WorkflowJob> {
        vec![
            job(1, "Tests 10"),
            job(2, "version"),
            job(3, "Desktop Linux / Build"),
            job(4, "Tests 1"),
            job(5, "Desktop Linux / Verify"),
            job(6, "publish"),
            job(7, "Tests 3"),
            job(8, "a job no file explains"),
        ]
    }

    fn now() -> DateTime<Utc> {
        "2026-11-11T11:00:00Z".parse().expect("now")
    }

    fn nodes() -> Vec<GraphNode> {
        graph_nodes(Some(&parse_workflow(WORKFLOW)), &jobs(), now())
    }

    #[test]
    fn a_matrix_and_a_called_workflow_each_get_one_box() {
        let nodes = nodes();
        let summary: Vec<(Option<String>, Vec<&str>, Vec<usize>)> = nodes
            .iter()
            .map(|node| {
                (
                    node.title(),
                    node.jobs.iter().map(|job| job.label.as_str()).collect(),
                    node.needs.clone(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (None, vec!["version"], vec![]),
                (
                    Some("Matrix: test".to_owned()),
                    vec!["Tests 1", "Tests 3", "Tests 10"],
                    vec![0]
                ),
                (
                    Some("Desktop Linux".to_owned()),
                    vec!["Build", "Verify"],
                    vec![0]
                ),
                (None, vec!["publish"], vec![1, 2]),
                (None, vec!["a job no file explains"], vec![]),
            ]
        );
        assert_eq!(nodes[1].jobs[0].seconds, Some(40));
        assert_eq!(
            nodes[1].height(true),
            TITLE_HEIGHT + 3. * JOB_HEIGHT + BOX_PADDING
        );
    }

    #[test]
    fn a_folded_matrix_is_one_line_and_a_called_workflow_never_folds() {
        let nodes = nodes();
        assert!(nodes[1].foldable());
        assert_eq!(
            nodes[1].height(false),
            TITLE_HEIGHT + JOB_HEIGHT + BOX_PADDING
        );
        assert!(!nodes[2].foldable());
        assert_eq!(nodes[2].height(false), nodes[2].height(true));
        assert_eq!(nodes[1].completed(), 3);
    }

    #[test]
    fn every_box_sits_right_of_the_boxes_it_needs() {
        let nodes = nodes();
        let layout = layout(&nodes, &[true; 5]);
        assert_eq!(layout.edges, [(0, 1), (0, 2), (1, 3), (2, 3)]);
        for (from, to) in &layout.edges {
            assert!(
                layout.boxes[*to].x > layout.boxes[*from].right(),
                "box {to} is not right of box {from}"
            );
        }
        // Boxes of one column never overlap.
        for (a, first) in layout.boxes.iter().enumerate() {
            for second in &layout.boxes[a + 1..] {
                let same_column = first.x == second.x;
                let apart = first.bottom() <= second.y || second.bottom() <= first.y;
                assert!(!same_column || apart, "{first:?} overlaps {second:?}");
            }
        }
        assert!(layout.boxes.iter().all(|b| b.x >= 0. && b.y >= 0.));
        // version, then the matrix and the called workflow, then publish.
        assert!(layout.width >= 3. * NODE_WIDTH);
    }

    #[test]
    fn a_box_that_needs_two_sits_level_between_them() {
        let nodes = nodes();
        let layout = layout(&nodes, &[true; 5]);
        let between = (layout.boxes[1].center_y() + layout.boxes[2].center_y()) / 2.;
        assert!((layout.boxes[3].center_y() - between).abs() < 1.);
    }

    #[test]
    fn folding_a_matrix_shortens_its_column() {
        let nodes = nodes();
        let open = layout(&nodes, &[true; 5]);
        let folded = layout(&nodes, &[true, false, true, true, true]);
        assert!(folded.boxes[1].height < open.boxes[1].height);
        assert!(folded.height <= open.height);
    }

    #[test]
    fn without_a_workflow_file_jobs_wrap_in_columns() {
        let nodes = graph_nodes(None, &jobs(), now());
        assert_eq!(nodes.len(), 8);
        assert!(
            nodes
                .iter()
                .all(|node| node.group.is_none() && node.needs.is_empty())
        );
        let layout = layout(&nodes, &[true; 8]);
        let columns: std::collections::BTreeSet<i64> =
            layout.boxes.iter().map(|b| b.x as i64).collect();
        assert_eq!(columns.len(), 2);
    }

    #[test]
    fn an_edge_leaves_rightward_and_arrives_from_the_left() {
        let layout = layout(&nodes(), &[true; 5]);
        let [start, a, b, end] = edge_curve(&layout.boxes[0], &layout.boxes[3]);
        assert_eq!(start.1, a.1);
        assert_eq!(end.1, b.1);
        assert!(a.0 > start.0 && b.0 < end.0);
        assert_eq!(start.0, layout.boxes[0].right());
        assert_eq!(end.0, layout.boxes[3].x);
    }
}
