//! The run graph on screen: the boxes of [`layout`] in columns from left to
//! right with a smooth curve from each box to the boxes that need it, a
//! duration on every job, and a canvas that zooms (the buttons, or Cmd/Ctrl
//! and the wheel), moves when dragged (and with the wheel in full screen),
//! and fills the panel in full screen. A matrix folds to one line that
//! counts its finished jobs and opens on a click. A click on a job opens its
//! log; every name cut short has its whole text in a tooltip.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::CheckStatus;

use super::jobs::group_status;
use super::layout::{
    GraphJob, GraphNode, JOB_HEIGHT, Layout, Placed, TITLE_HEIGHT, edge_curve, layout,
};
use super::{job_duration_label, status_style, status_word};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui;

const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 2.;
const ZOOM_STEP: f32 = 1.25;
/// Room around the graph when it is fitted to the view.
const FIT_MARGIN: f32 = 24.;
/// Minimum inline canvas height so short graphs stay compact and balanced.
const MIN_INLINE_HEIGHT: f32 = 104.;
/// Inline, a wide graph shrinks to fit the width down to this zoom, which
/// keeps its names readable; past it the graph is dragged sideways.
const INLINE_MIN_ZOOM: f32 = 0.72;
/// The inline height before the view knows its width.
const INLINE_HEIGHT: f32 = 140.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphEvent {
    OpenJob(u64),
    ToggleFullscreen,
}

pub struct GraphView {
    nodes: Vec<GraphNode>,
    /// When `nodes` came: a running job's time grows from there.
    loaded_at: Instant,
    layout: Layout,
    file_name: Option<String>,
    trigger: Option<String>,
    selected: Option<u64>,
    /// Matrices the user opened (`true`) or folded (`false`), by key; any
    /// other matrix is open while it holds the open job.
    folds: HashMap<String, bool>,
    /// Whether each box shows its jobs, as the layout was made.
    open: Vec<bool>,
    fullscreen: bool,
    zoom: f32,
    /// Where the graph's top-left corner is, from the view's.
    pan: Point<Pixels>,
    /// Whether the view has been fitted to the graph since its shape last
    /// changed; until it is, the next frame fits it.
    fitted: bool,
    /// The view's bounds as last painted, in the current mode.
    viewport: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Inline, the view is as tall as the graph at its fitted zoom.
    inline_height: Option<Pixels>,
    /// Where a drag started and the pan at that moment.
    drag: Option<(Point<Pixels>, Point<Pixels>)>,
}

impl EventEmitter<GraphEvent> for GraphView {}

impl GraphView {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            loaded_at: Instant::now(),
            layout: Layout::default(),
            file_name: None,
            trigger: None,
            selected: None,
            folds: HashMap::new(),
            open: Vec::new(),
            fullscreen: false,
            zoom: 1.,
            pan: point(px(0.), px(0.)),
            fitted: false,
            viewport: Rc::new(Cell::new(None)),
            inline_height: None,
            drag: None,
        }
    }

    /// Shows `nodes`. The layout and the view's fit change only when the
    /// graph's shape does, so a run that updates while it goes keeps the
    /// user's zoom and position.
    pub fn set_nodes(&mut self, nodes: Vec<GraphNode>, cx: &mut Context<Self>) {
        let shape = |nodes: &[GraphNode]| -> Vec<(Option<String>, Vec<u64>, Vec<usize>)> {
            nodes
                .iter()
                .map(|node| {
                    (
                        node.title(),
                        node.jobs.iter().map(|job| job.id).collect(),
                        node.needs.clone(),
                    )
                })
                .collect()
        };
        let changed = shape(&nodes) != shape(&self.nodes);
        self.nodes = nodes;
        self.loaded_at = Instant::now();
        if changed {
            self.open.clear();
        }
        self.relayout();
        cx.notify();
    }

    /// Whether each box shows its jobs now.
    fn open_boxes(&self) -> Vec<bool> {
        self.nodes
            .iter()
            .map(|node| {
                !node.foldable()
                    || node
                        .group_name()
                        .and_then(|name| self.folds.get(name).copied())
                        .unwrap_or_else(|| self.selected.is_some_and(|job| node.contains(job)))
            })
            .collect()
    }

    /// Lays the graph out again when a box opened or folded, or the shape
    /// changed; the view then fits it again.
    fn relayout(&mut self) {
        let open = self.open_boxes();
        if open != self.open {
            self.layout = layout(&self.nodes, &open);
            self.open = open;
            self.fitted = false;
        }
    }

    fn toggle_box(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(name) = self
            .nodes
            .get(ix)
            .and_then(|node| node.group_name())
            .map(str::to_owned)
        else {
            return;
        };
        let open = self.open.get(ix).copied().unwrap_or(true);
        self.folds.insert(name, !open);
        self.relayout();
        cx.notify();
    }

    pub fn set_meta(&mut self, file_name: String, trigger: String, cx: &mut Context<Self>) {
        if self.file_name.as_deref() != Some(&file_name)
            || self.trigger.as_deref() != Some(&trigger)
        {
            self.file_name = Some(file_name);
            self.trigger = Some(trigger);
            cx.notify();
        }
    }

    pub fn set_selected(&mut self, job: Option<u64>, cx: &mut Context<Self>) {
        if self.selected != job {
            self.selected = job;
            self.relayout();
            cx.notify();
        }
    }

    pub fn set_fullscreen(&mut self, fullscreen: bool, cx: &mut Context<Self>) {
        if self.fullscreen != fullscreen {
            self.fullscreen = fullscreen;
            self.fitted = false;
            // The bounds of the other mode would fit the wrong size.
            self.viewport.set(None);
            cx.notify();
        }
    }

    /// Zooms to show the graph, at most at 100%, centered. In full screen
    /// the whole graph fits; inline it fits the width (down to
    /// [`INLINE_MIN_ZOOM`]) and the view takes the graph's height.
    fn fit(&mut self, size: Size<Pixels>) {
        let (width, height) = (f32::from(size.width), f32::from(size.height));
        let graph_width = dp(self.layout.width).as_f32() + 2. * FIT_MARGIN;
        let graph_height = dp(self.layout.height).as_f32() + 2. * FIT_MARGIN;
        if graph_width <= 0. || width <= 0. || height <= 0. {
            return;
        }
        self.zoom = if self.fullscreen {
            (width / graph_width)
                .min(height / graph_height)
                .clamp(MIN_ZOOM, 1.)
        } else {
            (width / graph_width).clamp(INLINE_MIN_ZOOM, 1.)
        };
        let shown_width = dp(self.layout.width).as_f32() * self.zoom;
        let shown_height = dp(self.layout.height).as_f32() * self.zoom;
        let (left, top) = if self.fullscreen {
            (
                ((width - shown_width) / 2.).max(FIT_MARGIN),
                ((height - shown_height) / 2.).max(FIT_MARGIN),
            )
        } else {
            let canvas_height = (shown_height + 2. * FIT_MARGIN).max(MIN_INLINE_HEIGHT);
            self.inline_height = Some(px(canvas_height));
            // Align near the leading edge on wide viewports so a small graph
            // does not float in the middle of an empty expanse; center when
            // tight so narrow viewports keep balanced margins without clipping.
            let left = if width >= shown_width + 2. * FIT_MARGIN {
                FIT_MARGIN
            } else {
                ((width - shown_width) / 2.).max(12.)
            };
            let top = ((canvas_height - shown_height) / 2.).max(FIT_MARGIN);
            (left, top)
        };
        self.pan = point(px(left), px(top));
        self.fitted = true;
    }

    /// Zooms by `factor` keeping the point `anchor` (from the view's corner)
    /// where it is.
    fn zoom_at(&mut self, factor: f32, anchor: Point<Pixels>) {
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let applied = zoom / self.zoom;
        self.pan = point(
            anchor.x - (anchor.x - self.pan.x) * applied,
            anchor.y - (anchor.y - self.pan.y) * applied,
        );
        self.zoom = zoom;
    }

    fn zoom_center(&mut self, factor: f32, cx: &mut Context<Self>) {
        let center = self.viewport.get().map_or(point(px(0.), px(0.)), |bounds| {
            point(bounds.size.width / 2., bounds.size.height / 2.)
        });
        self.zoom_at(factor, center);
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(window.line_height());
        if event.modifiers.platform || event.modifiers.control {
            let origin = self
                .viewport
                .get()
                .map_or(point(px(0.), px(0.)), |bounds| bounds.origin);
            let factor = (1. + f32::from(delta.y) / 400.).clamp(0.5, 2.);
            self.zoom_at(factor, event.position - origin);
        } else if self.fullscreen {
            self.pan += delta;
        } else {
            // Inline, the wheel scrolls the page around the graph.
            return;
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// One job's row: its state, its name, its time. `in_box` rows sit
    /// inside a box's border and have one of their own.
    fn job_row(&self, job: &GraphJob, in_box: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let zoom = self.zoom;
        let z = |size: f32| dp(size * zoom);
        let theme = cx.theme();
        let (icon, color) = status_style(job.status);
        let selected = self.selected == Some(job.id);
        let border = if selected {
            tokens::accent()
        } else {
            theme.border
        };
        let id = job.id;
        let seconds = match job.status {
            CheckStatus::Running => job
                .seconds
                .map(|seconds| seconds + self.loaded_at.elapsed().as_secs() as i64),
            _ => job.seconds,
        };
        let mut tip = format!("{} · {}", job.name, status_word(job.status));
        if let Some(time) = job_duration_label(job.status, seconds) {
            tip.push_str(&format!(" · {time}"));
        }
        h_flex()
            .id(SharedString::from(format!("graph-job-{id}")))
            .role(Role::Button)
            .aria_label(format!("{}, open its log", job.name))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .h(z(JOB_HEIGHT - if in_box { 6. } else { 0. }))
            .when(in_box, |row| row.mx(z(6.)).mt(z(6.)))
            .px(z(10.))
            .gap(z(8.))
            .items_center()
            .rounded(z(6.))
            .border_1()
            .border_color(border)
            .bg(theme.group_box)
            .hover(|row| row.bg(theme.list_hover))
            .text_size(z(12.))
            .child(Icon::new(icon).size(z(14.)).flex_none().text_color(color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(job.label.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(theme.muted_foreground)
                    .children(job_duration_label(job.status, seconds)),
            )
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(GraphEvent::OpenJob(id))))
    }

    fn node(
        &self,
        ix: usize,
        node: &GraphNode,
        placed: Placed,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = self.zoom;
        let z = |size: f32| dp(size * zoom);
        let positioned = div()
            .absolute()
            .left(self.pan.x + z(placed.x))
            .top(self.pan.y + z(placed.y))
            .w(z(placed.width));
        let Some(title) = node.title() else {
            return positioned
                .children(node.jobs.iter().map(|job| self.job_row(job, false, cx)))
                .into_any_element();
        };
        let foldable = node.foldable();
        let open = self.open.get(ix).copied().unwrap_or(true);
        let (muted, hover) = (cx.theme().muted_foreground, cx.theme().list_hover);
        let heading = h_flex()
            .id(("graph-box-title", ix))
            .h(z(TITLE_HEIGHT - 6.))
            .mt(z(6.))
            .mx(z(4.))
            .px(z(6.))
            .gap(z(4.))
            .items_center()
            .rounded(z(4.))
            .text_size(z(12.))
            .text_color(muted)
            .when(foldable, |row| {
                row.child(
                    Icon::new(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(z(12.))
                    .flex_none()
                    .text_color(muted),
                )
            })
            .child(div().flex_1().min_w_0().truncate().child(title.clone()))
            .tooltip({
                let title = title.clone();
                move |window, cx| Tooltip::new(title.clone()).build(window, cx)
            })
            .when(foldable, |row| {
                row.role(Role::Button)
                    .aria_label(format!(
                        "{title}, {}",
                        if open {
                            "fold its jobs"
                        } else {
                            "show its jobs"
                        }
                    ))
                    .aria_expanded(open)
                    .hover(move |row| row.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_box(ix, cx)))
            });
        let body: Vec<AnyElement> = if open || !foldable {
            node.jobs
                .iter()
                .map(|job| self.job_row(job, true, cx).into_any_element())
                .collect()
        } else {
            vec![self.folded_row(ix, node, cx).into_any_element()]
        };
        let theme = cx.theme();
        positioned
            .child(
                v_flex()
                    .id(("graph-box", ix))
                    .role(Role::Group)
                    .aria_label(title)
                    .h(z(placed.height))
                    .rounded(z(8.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(tokens::panel())
                    .child(heading)
                    .children(body),
            )
            .into_any_element()
    }

    /// A folded matrix's one line: its state as one, and how many of its
    /// jobs finished. A click shows its jobs.
    fn folded_row(&self, ix: usize, node: &GraphNode, cx: &mut Context<Self>) -> Stateful<Div> {
        let zoom = self.zoom;
        let z = |size: f32| dp(size * zoom);
        let theme = cx.theme();
        let (border, fill, hover, muted) = (
            theme.border,
            theme.group_box,
            theme.list_hover,
            theme.muted_foreground,
        );
        let (icon, color) = status_style(group_status(&node.jobs));
        let selected = self.selected.is_some_and(|job| node.contains(job));
        let text = format!("{}/{} jobs completed", node.completed(), node.jobs.len());
        h_flex()
            .id(("graph-box-folded", ix))
            .role(Role::Button)
            .aria_label(format!("{text}, show its jobs"))
            .h(z(JOB_HEIGHT - 6.))
            .mx(z(6.))
            .mt(z(6.))
            .px(z(10.))
            .gap(z(8.))
            .items_center()
            .rounded(z(6.))
            .border_1()
            .border_color(if selected { tokens::accent() } else { border })
            .bg(fill)
            .hover(move |row| row.bg(hover))
            .text_size(z(12.))
            .child(Icon::new(icon).size(z(14.)).flex_none().text_color(color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(text),
            )
            .child(div().flex_none().text_color(muted).child("Show all"))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_box(ix, cx)))
    }

    fn controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let percent = (self.zoom * 100.).round() as u32;
        h_flex()
            .flex_none()
            .gap_0p5()
            .items_center()
            .child(
                ui::icon_button("graph-zoom-out", IconName::Minus, "Zoom out")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_center(1. / ZOOM_STEP, cx))),
            )
            .child(
                Button::new("graph-zoom-reset")
                    .ghost()
                    .small()
                    .label(format!("{percent}%"))
                    .tooltip("Zoom to 100%")
                    .accessibility_label(format!("Zoom {percent}%, reset to 100%"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let factor = 1. / this.zoom;
                        this.zoom_center(factor, cx)
                    })),
            )
            .child(
                ui::icon_button("graph-zoom-in", IconName::Plus, "Zoom in")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_center(ZOOM_STEP, cx))),
            )
            .child(ui::vertical_divider(cx))
            .child(
                ui::icon_button("graph-fit", IconName::Focus, "Fit to view").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.fitted = false;
                        cx.notify();
                    },
                )),
            )
            .child(
                ui::icon_button(
                    "graph-fullscreen",
                    if self.fullscreen {
                        IconName::Minimize2
                    } else {
                        IconName::Maximize2
                    },
                    if self.fullscreen {
                        "Exit full screen"
                    } else {
                        "Full screen"
                    },
                )
                .on_click(cx.listener(|_, _, _, cx| cx.emit(GraphEvent::ToggleFullscreen))),
            )
    }

    fn header_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let file = self
            .file_name
            .clone()
            .unwrap_or_else(|| "Workflow".to_owned());
        h_flex()
            .flex_none()
            .w_full()
            .h(scale::band_height())
            .px_3()
            .gap_2()
            .items_center()
            .bg(theme.group_box)
            .border_b_1()
            .border_color(tokens::border_subtle())
            .child(
                Icon::new(IconName::FileCode)
                    .small()
                    .flex_none()
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(theme.mono_font_family.clone())
                    .text_size(scale::code())
                    .font_weight(FontWeight::MEDIUM)
                    .child(file),
            )
            .when_some(self.trigger.clone(), |bar, trigger| {
                bar.child(
                    div()
                        .flex_none()
                        .text_size(scale::meta())
                        .text_color(theme.muted_foreground)
                        .child(format!("· on: {trigger}")),
                )
            })
            .child(div().flex_1())
            .child(self.controls(cx))
    }
}

impl Render for GraphView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.nodes.is_empty() {
            return div()
                .p(scale::content_inset())
                .child(ui::empty_line(
                    IconName::Workflow,
                    "No jobs yet. The graph appears when GitHub queues them.",
                    cx,
                ))
                .into_any_element();
        }
        if !self.fitted
            && let Some(bounds) = self.viewport.get()
        {
            self.fit(bounds.size);
        }
        let zoom = self.zoom;
        let pan = self.pan;
        let curves: Vec<[(f32, f32); 4]> = self
            .layout
            .edges
            .iter()
            .filter_map(|(from, to)| {
                Some(edge_curve(
                    self.layout.boxes.get(*from)?,
                    self.layout.boxes.get(*to)?,
                ))
            })
            .collect();
        let edge_color = cx.theme().muted_foreground.opacity(0.55);
        let dot_color = tokens::border_subtle();
        let viewport = self.viewport.clone();
        let needs_fit = !self.fitted;
        let edges = canvas(
            move |bounds, _, _| {
                viewport.set(Some(bounds));
                bounds
            },
            move |_, bounds, window, _| {
                // Subtle dot grid signals an interactive pan/zoom canvas.
                let step = dp(20. * zoom)
                    .as_f32()
                    .clamp(dp(16.).as_f32(), dp(36.).as_f32());
                let width = f32::from(bounds.size.width);
                let height = f32::from(bounds.size.height);
                let ox = f32::from(pan.x).rem_euclid(step);
                let oy = f32::from(pan.y).rem_euclid(step);
                let dot = size(dp(1.5), dp(1.5));
                let mut y = oy;
                while y < height {
                    let mut x = ox;
                    while x < width {
                        window.paint_quad(fill(
                            Bounds::new(
                                point(bounds.origin.x + px(x), bounds.origin.y + px(y)),
                                dot,
                            ),
                            dot_color,
                        ));
                        x += step;
                    }
                    y += step;
                }
                let at = |(x, y): (f32, f32)| {
                    point(
                        bounds.origin.x + pan.x + dp(x * zoom),
                        bounds.origin.y + pan.y + dp(y * zoom),
                    )
                };
                for [start, a, b, end] in &curves {
                    let mut path = PathBuilder::stroke(dp(1.5 * zoom.max(0.5)));
                    path.move_to(at(*start));
                    path.cubic_bezier_to(at(*end), at(*a), at(*b));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, edge_color);
                    }
                }
                // The first frame learns the view's size; fit on the next.
                if needs_fit {
                    window.refresh();
                }
            },
        )
        .absolute()
        .size_full();
        let boxes = self.layout.boxes.clone();
        let nodes: Vec<AnyElement> = self
            .nodes
            .clone()
            .iter()
            .zip(boxes)
            .enumerate()
            .map(|(ix, (node, placed))| self.node(ix, node, placed, cx))
            .collect();
        let canvas_area = div()
            .id("run-graph")
            .role(Role::Group)
            .aria_label("Run graph")
            .relative()
            .bg(tokens::background().opacity(0.45))
            .when(self.fullscreen, |view| view.flex_1().w_full().min_h_0())
            .when(!self.fullscreen, |view| {
                view.w_full()
                    .h(self.inline_height.unwrap_or(dp(INLINE_HEIGHT)))
            })
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    this.drag = Some((event.position, this.pan));
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if let (Some((start, pan)), true) = (this.drag, event.dragging()) {
                    this.pan = pan + (event.position - start);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.drag = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.drag = None),
            )
            .on_scroll(cx.listener(Self::on_scroll))
            .child(edges)
            .children(nodes)
            .when(self.fullscreen, |canvas| {
                canvas.child(
                    div()
                        .absolute()
                        .bottom_2()
                        .left_3()
                        .text_size(scale::meta())
                        .text_color(tokens::text_faint())
                        .child("Drag or scroll to move · Cmd or Ctrl and scroll to zoom"),
                )
            });
        v_flex()
            .when(self.fullscreen, |view| view.size_full())
            .when(!self.fullscreen, |view| view.w_full())
            .child(self.header_bar(cx))
            .child(canvas_area)
            .into_any_element()
    }
}
