//! The run graph on screen: the boxes of [`layout`] from top to bottom with a
//! smooth curve from each box to the boxes that need it, a duration on every
//! job, and a canvas that zooms (the buttons, or Cmd/Ctrl and the wheel),
//! moves when dragged (and with the wheel in full screen), and fills the
//! panel in full screen. A click on a job opens its log.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::layout::{
    GraphJob, GraphNode, JOB_HEIGHT, Layout, Placed, TITLE_HEIGHT, edge_curve, layout,
};
use super::{job_duration_label, status_style};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};
use crate::ui;

const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 2.;
const ZOOM_STEP: f32 = 1.25;
/// Room around the graph when it is fitted to the view.
const FIT_MARGIN: f32 = 24.;
/// Inline, a wide graph shrinks to fit the width down to this zoom, which
/// keeps its names readable; past it the graph is dragged sideways.
const INLINE_MIN_ZOOM: f32 = 0.6;
/// The inline height before the view knows its width.
const INLINE_HEIGHT: f32 = 480.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphEvent {
    OpenJob(u64),
    ToggleFullscreen,
}

pub struct GraphView {
    nodes: Vec<GraphNode>,
    layout: Layout,
    selected: Option<u64>,
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
            layout: Layout::default(),
            selected: None,
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
                        node.title.clone(),
                        node.jobs.iter().map(|job| job.id).collect(),
                        node.needs.clone(),
                    )
                })
                .collect()
        };
        if shape(&nodes) != shape(&self.nodes) {
            self.layout = layout(&nodes);
            self.fitted = false;
        }
        self.nodes = nodes;
        cx.notify();
    }

    pub fn set_selected(&mut self, job: Option<u64>, cx: &mut Context<Self>) {
        if self.selected != job {
            self.selected = job;
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
        let top = if self.fullscreen {
            ((height - shown_height) / 2.).max(FIT_MARGIN)
        } else {
            self.inline_height = Some(px(shown_height + 2. * FIT_MARGIN));
            FIT_MARGIN
        };
        self.pan = point(px((width - shown_width) / 2.), px(top));
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
        h_flex()
            .id(SharedString::from(format!("graph-job-{id}")))
            .role(Role::Button)
            .aria_label(format!("{}, open its log", job.name))
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
                    .children(job_duration_label(job.status, job.seconds)),
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
        match &node.title {
            None => positioned
                .children(node.jobs.iter().map(|job| self.job_row(job, false, cx)))
                .into_any_element(),
            Some(title) => {
                let rows: Vec<Stateful<Div>> = node
                    .jobs
                    .iter()
                    .map(|job| self.job_row(job, true, cx))
                    .collect();
                let theme = cx.theme();
                positioned
                    .child(
                        v_flex()
                            .id(("graph-box", ix))
                            .role(Role::Group)
                            .aria_label(title.clone())
                            .h(z(placed.height))
                            .rounded(z(8.))
                            .border_1()
                            .border_color(theme.border)
                            .bg(tokens::panel())
                            .child(
                                div()
                                    .h(z(TITLE_HEIGHT - 6.))
                                    .mt(z(6.))
                                    .px(z(10.))
                                    .truncate()
                                    .text_size(z(12.))
                                    .text_color(theme.muted_foreground)
                                    .child(title.clone()),
                            )
                            .children(rows),
                    )
                    .into_any_element()
            }
        }
    }

    fn controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let percent = (self.zoom * 100.).round() as u32;
        h_flex()
            .absolute()
            .top_2()
            .right_2()
            .p_0p5()
            .gap_0p5()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .child(
                ui::icon_button("graph-zoom-out", IconName::ZoomOut, "Zoom out")
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
                ui::icon_button("graph-zoom-in", IconName::ZoomIn, "Zoom in")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_center(ZOOM_STEP, cx))),
            )
            .child(
                ui::icon_button("graph-fit", IconName::Scan, "Fit to view").on_click(cx.listener(
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
                        IconName::Minimize
                    } else {
                        IconName::Maximize
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
        let viewport = self.viewport.clone();
        let needs_fit = !self.fitted;
        let edges = canvas(
            move |bounds, _, _| {
                viewport.set(Some(bounds));
                bounds
            },
            move |_, bounds, window, _| {
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
        div()
            .id("run-graph")
            .role(Role::Group)
            .aria_label("Run graph")
            .relative()
            .when(self.fullscreen, |view| view.size_full())
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
            .child(self.controls(cx))
            .child(
                div()
                    .absolute()
                    .bottom_2()
                    .left(scale::content_inset())
                    .text_size(scale::meta())
                    .text_color(tokens::text_faint())
                    .child(if self.fullscreen {
                        "Drag or scroll to move · Cmd or Ctrl and scroll to zoom"
                    } else {
                        "Drag to move · Cmd or Ctrl and scroll to zoom"
                    }),
            )
            .into_any_element()
    }
}
