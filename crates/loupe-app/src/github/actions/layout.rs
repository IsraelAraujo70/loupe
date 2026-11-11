//! The run graph's shape, without GPUI: which jobs share a box (a matrix, a
//! called workflow), which boxes need which, and where each box goes when
//! the graph runs from top to bottom. Sizes are at 100% zoom.

use chrono::{DateTime, Utc};
use loupe_github::{CheckStatus, WorkflowJob, WorkflowSpec, assign_jobs, called_job_name};

pub const NODE_WIDTH: f32 = 240.;
/// One job's row, alone or inside a box.
pub const JOB_HEIGHT: f32 = 32.;
/// A box's title over its jobs.
pub const TITLE_HEIGHT: f32 = 24.;
/// Room under the last job of a box.
pub const BOX_PADDING: f32 = 6.;
const COLUMN_GAP: f32 = 24.;
const ROW_GAP: f32 = 64.;
/// Without a workflow file there are no edges; the jobs wrap at this many
/// per row instead of standing in one long row.
const GRID_COLUMNS: usize = 4;

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

/// A box of the graph: one job, or the jobs of one matrix or one called
/// workflow under a title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphNode {
    pub title: Option<String>,
    pub jobs: Vec<GraphJob>,
    /// The boxes it needs, by index.
    pub needs: Vec<usize>,
}

impl GraphNode {
    pub fn height(&self) -> f32 {
        let rows = self.jobs.len() as f32 * JOB_HEIGHT;
        match self.title {
            Some(_) => TITLE_HEIGHT + rows + BOX_PADDING,
            None => rows,
        }
    }
}

/// The boxes of a run: one per job of the workflow file that ran, in the
/// file's order, with what it needs; a job the file does not explain stands
/// alone. A matrix's box is titled `Matrix: <key>`, a called workflow's with
/// its name.
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
        let title = match () {
            _ if unexpanded => None,
            _ if spec_job.reusable => Some(spec_job.display_name().to_owned()),
            _ if spec_job.matrix || members.len() > 1 => Some(format!("Matrix: {}", spec_job.key)),
            _ => None,
        };
        node_of_spec[spec_ix] = Some(nodes.len());
        nodes.push(GraphNode {
            title,
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
                title: None,
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
    pub fn center_x(&self) -> f32 {
        self.x + self.width / 2.
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

/// The ends and control points of an edge's curve: it leaves the bottom of
/// the box it comes from and enters the top of the one that needs it, both
/// vertically, so the line is a smooth S.
pub fn edge_curve(from: &Placed, to: &Placed) -> [(f32, f32); 4] {
    let start = (from.center_x(), from.bottom());
    let end = (to.center_x(), to.y);
    let bend = ((end.1 - start.1) / 2.).max(24.);
    [start, (start.0, start.1 + bend), (end.0, end.1 - bend), end]
}

/// Lays the boxes out from top to bottom: each box one row under the
/// lowest box it needs, rows ordered to keep a box near what it needs, and
/// each row centered under its parents.
pub fn layout(nodes: &[GraphNode]) -> Layout {
    let count = nodes.len();
    if count == 0 {
        return Layout::default();
    }
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
        (0..count).map(|ix| ix / GRID_COLUMNS).collect()
    } else {
        ranks(nodes)
    };
    let depth = rank.iter().copied().max().unwrap_or(0) + 1;
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); depth];
    for (ix, rank) in rank.iter().enumerate() {
        rows[*rank].push(ix);
    }
    order_rows(&mut rows, nodes);

    let mut boxes = vec![
        Placed {
            x: 0.,
            y: 0.,
            width: NODE_WIDTH,
            height: 0.,
        };
        count
    ];
    let mut y = 0.;
    for row in &rows {
        let row_height = row.iter().map(|ix| nodes[*ix].height()).fold(0., f32::max);
        // Where each box would like its center: under its parents' centers.
        let wanted: Vec<f32> = row
            .iter()
            .enumerate()
            .map(|(slot, ix)| {
                let parents: Vec<f32> = nodes[*ix]
                    .needs
                    .iter()
                    .filter(|need| rank[**need] < rank[*ix])
                    .map(|need| boxes[*need].center_x())
                    .collect();
                if parents.is_empty() || y == 0. {
                    slot as f32 * (NODE_WIDTH + COLUMN_GAP)
                } else {
                    parents.iter().sum::<f32>() / parents.len() as f32 - NODE_WIDTH / 2.
                }
            })
            .collect();
        let mut x: Vec<f32> = Vec::with_capacity(row.len());
        for want in &wanted {
            let floor = x
                .last()
                .map_or(f32::MIN, |left: &f32| left + NODE_WIDTH + COLUMN_GAP);
            x.push(want.max(floor));
        }
        // Pushing right drifts the row; slide it back so it sits as close to
        // where it wanted to be on average.
        let drift = wanted.iter().zip(&x).map(|(w, x)| x - w).sum::<f32>() / row.len() as f32;
        for (slot, ix) in row.iter().enumerate() {
            boxes[*ix] = Placed {
                x: x[slot] - drift,
                y,
                width: NODE_WIDTH,
                height: nodes[*ix].height(),
            };
        }
        y += row_height + ROW_GAP;
    }
    let left = boxes.iter().map(|b| b.x).fold(f32::MAX, f32::min);
    for placed in &mut boxes {
        placed.x -= left;
    }
    let width = boxes.iter().map(|b| b.x + b.width).fold(0., f32::max);
    let height = boxes.iter().map(Placed::bottom).fold(0., f32::max);
    Layout {
        boxes,
        edges,
        width,
        height,
    }
}

/// Each box's row: one under the lowest box it needs. A cycle (which GitHub
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

/// Orders each row by the mean slot of the boxes it needs (going down) and
/// then of the boxes that need it (going up), a few times, which keeps
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

    use super::{GraphNode, JOB_HEIGHT, NODE_WIDTH, TITLE_HEIGHT, edge_curve, graph_nodes, layout};

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
        let summary: Vec<(Option<&str>, Vec<&str>, Vec<usize>)> = nodes
            .iter()
            .map(|node| {
                (
                    node.title.as_deref(),
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
                    Some("Matrix: test"),
                    vec!["Tests 1", "Tests 3", "Tests 10"],
                    vec![0]
                ),
                (Some("Desktop Linux"), vec!["Build", "Verify"], vec![0]),
                (None, vec!["publish"], vec![1, 2]),
                (None, vec!["a job no file explains"], vec![]),
            ]
        );
        assert_eq!(nodes[1].jobs[0].seconds, Some(40));
        assert_eq!(
            nodes[1].height(),
            TITLE_HEIGHT + 3. * JOB_HEIGHT + super::BOX_PADDING
        );
    }

    #[test]
    fn every_box_sits_below_the_boxes_it_needs() {
        let nodes = nodes();
        let layout = layout(&nodes);
        assert_eq!(layout.edges, [(0, 1), (0, 2), (1, 3), (2, 3)]);
        for (from, to) in &layout.edges {
            assert!(
                layout.boxes[*to].y > layout.boxes[*from].bottom(),
                "box {to} is not below box {from}"
            );
        }
        // Boxes of one row never overlap.
        for (a, first) in layout.boxes.iter().enumerate() {
            for second in &layout.boxes[a + 1..] {
                let same_row = first.y == second.y;
                let apart = first.x + first.width <= second.x || second.x + second.width <= first.x;
                assert!(!same_row || apart, "{first:?} overlaps {second:?}");
            }
        }
        assert!(layout.boxes.iter().all(|b| b.x >= 0. && b.y >= 0.));
        assert!(layout.width >= 2. * NODE_WIDTH);
    }

    #[test]
    fn a_box_needed_by_one_box_sits_above_it() {
        let nodes = nodes();
        let layout = layout(&nodes);
        // publish needs the matrix and the called workflow: it is centered
        // between them.
        let between = (layout.boxes[1].center_x() + layout.boxes[2].center_x()) / 2.;
        assert!((layout.boxes[3].center_x() - between).abs() < 1.);
    }

    #[test]
    fn without_a_workflow_file_jobs_wrap_in_rows() {
        let nodes = graph_nodes(None, &jobs(), now());
        assert_eq!(nodes.len(), 8);
        assert!(
            nodes
                .iter()
                .all(|node| node.title.is_none() && node.needs.is_empty())
        );
        let layout = layout(&nodes);
        let rows: std::collections::BTreeSet<i64> =
            layout.boxes.iter().map(|b| b.y as i64).collect();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn an_edge_leaves_downward_and_arrives_from_above() {
        let layout = layout(&nodes());
        let [start, a, b, end] = edge_curve(&layout.boxes[0], &layout.boxes[3]);
        assert_eq!(start.0, a.0);
        assert_eq!(end.0, b.0);
        assert!(a.1 > start.1 && b.1 < end.1);
    }
}
